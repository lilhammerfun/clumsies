import Combine
import Foundation

@MainActor
final class ReviewsModel: ObservableObject {
    private let context: WorkspaceContext
    private let edits: DraftStore
    private let feedback: WorkspaceFeedback
    private let navigation: WorkspaceNavigation
    private let reconciliation: DraftReconciliationService
    private let sessions: DocumentSessions
    var onMerged: (() async -> Void)?

    init(context: WorkspaceContext, edits: DraftStore, feedback: WorkspaceFeedback, navigation: WorkspaceNavigation, reconciliation: DraftReconciliationService, sessions: DocumentSessions) {
        self.context = context
        self.edits = edits
        self.feedback = feedback
        self.navigation = navigation
        self.reconciliation = reconciliation
        self.sessions = sessions
    }

    @Published var reviews: [ReviewRecord] = []
    @Published var reviewLoadState: WorkspaceCollectionLoadState = .loading
    @Published var selectedReviewId: String?
    @Published var pendingReviewReconciliationId: String?
    @Published var reviewDecisionReadiness: ReviewDecisionReadiness?

    private var reviewLoadTask: Task<Void, Never>?

    var selectedReview: ReviewRecord? {
        reviews.first { $0.id == self.selectedReviewId } ?? reviews.first
    }

    func review(for draft: LocalDraft) -> ReviewRecord? {
        guard draft.status == .submitted, let serverId = draft.serverId else { return nil }
        return reviews.first {
            $0.projectId == draft.projectId
                && ($0.status == "open" || $0.status == "approved")
                && ($0.draftId == serverId || $0.draftIds.contains(serverId))
        }
    }

    func openReview(_ review: ReviewRecord) {
        selectedReviewId = review.id
        navigation.selectedSection = .reviews
    }

    func openReview(for draft: LocalDraft) async {
        let generation = context.workspaceReloadGeneration
        let projectId = context.activeProjectId
        let section = navigation.selectedSection
        do {
            if review(for: draft) == nil {
                let baseline = reviews
                let loaded = try await WorkspaceLoader(
                    daemon: context.daemon, bootstrap: context.bootstrap, server: context.server
                ).loadReviews()
                guard context.workspaceReloadGeneration == generation,
                      context.activeProjectId == projectId,
                      navigation.selectedSection == section, !Task.isCancelled else { return }
                reviews = WorkspaceLoadPolicy.mergeDeferredRecords(
                    baseline: baseline, current: reviews, loaded: loaded.records
                )
                reviewLoadState = .loaded
            }
            guard let review = review(for: draft) else {
                feedback.errorMessage = "The Review for this draft is unavailable. It may have already been closed."
                return
            }
            openReview(review)
        } catch {
            guard context.workspaceReloadGeneration == generation,
                  context.activeProjectId == projectId,
                  navigation.selectedSection == section, !Task.isCancelled else { return }
            feedback.errorMessage = error.localizedDescription
        }
    }

    var submittedProjectDrafts: [LocalDraft] {
        guard let activeProjectId = context.activeProjectId else { return [] }
        return edits.drafts.filter { $0.projectId == activeProjectId && $0.status == .submitted }
    }

    func canPerformReviewMenuAction(_ action: ReviewMenuAction) -> Bool {
        guard context.phase == .ready,
              navigation.selectedSection == .reviews,
              let selectedReviewId = selectedReviewId,
              let review = reviews.first(where: { $0.id == selectedReviewId }),
              reviewDecisionReadiness?.matches(review) == true,
              review.freshness == .current else {
            return false
        }
        return action.isAvailable(
            for: review,
            canDecideReviews: context.canDecideReviews,
            canMergeReviews: context.canMergeReviews,
            isAuthor: context.isReviewAuthor(review)
        )
    }

    func performReviewMenuAction(_ action: ReviewMenuAction) async {
        guard canPerformReviewMenuAction(action),
              let selectedReviewId = selectedReviewId,
              let review = reviews.first(where: { $0.id == selectedReviewId }),
              let readiness = reviewDecisionReadiness else { return }
        reviewDecisionReadiness = nil
        let authority = context.authorityGeneration
        do {
            switch action {
            case .approve:
                try await merge(review)
            case .reject:
                try await decide(review, decision: "rejected", note: "")
            case .merge:
                try await merge(review)
            case .resubmit:
                let detail = try await reviewDetail(review.id)
                if detail.draft.coordination.freshness == .behind {
                    pendingReviewReconciliationId = review.id
                } else {
                    try await resubmit(review, detail: detail)
                }
            }
        } catch {
            guard context.authorityGeneration == authority, !(error is CancellationError) else { return }
            if self.selectedReviewId == selectedReviewId,
               navigation.selectedSection == .reviews,
               let currentReview = reviews.first(where: { $0.id == selectedReviewId }),
               readiness.matches(currentReview) {
                reviewDecisionReadiness = readiness
            }
            feedback.errorMessage = error.localizedDescription
        }
    }

    func requestReview(
        for draft: LocalDraft,
        title: String,
        description: String,
        candidate: DraftReconciliationCandidate? = nil,
        resolvedState: ReconciliationResourceState? = nil
    ) async throws {
        let authority = context.authorityGeneration
        if sessions.synchronizationItemId(for: draft) != nil {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        guard Self.canRequestReview(draft) else {
            throw ReviewRequestError.legacyProjectDraftCannotBePublished
        }
        guard let serverId = draft.serverId else {
            throw ReviewRequestError.draftNotSynchronized
        }
        guard draft.freshness == .current || candidate != nil else {
            throw ReviewRequestError.reconciliationRequired
        }
        let detail: ReviewDetail = try await context.server.send(
            method: "POST",
            path: "/api/v1/reviews",
            headers: ["If-Match": DraftReconciliationService.refETag(candidate?.currentCommitId ?? draft.currentCommitId)],
            body: CreateReviewRequest(
                drafts: [ReviewDraftRequest(
                    draftId: serverId,
                    expectedDraftVersion: candidate?.draftVersion ?? draft.serverVersion,
                    candidateId: candidate?.candidateId,
                    resolvedState: resolvedState
                )],
                title: title,
                description: description
            )
        )
        try context.ensureAuthority(authority)
        let record = WorkspaceLoader.mapReview(detail)
        reviews.insert(record, at: 0)
        selectedReviewId = record.id
        navigation.selectedSection = .reviews
    }

    func requestReview(
        for drafts: [LocalDraft],
        title: String,
        description: String,
        reconciliations: [ReviewDraftReconciliation] = []
    ) async throws {
        let authority = context.authorityGeneration
        let selectedDrafts = drafts.sorted {
            $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending
        }
        guard let selectedPrimary = selectedDrafts.first else { return }
        guard selectedDrafts.allSatisfy({ $0.projectId == selectedPrimary.projectId }) else {
            throw ReviewRequestError.mixedProjects
        }
        guard selectedDrafts.allSatisfy(Self.canRequestReview) else {
            throw ReviewRequestError.legacyProjectDraftCannotBePublished
        }
        guard selectedDrafts.allSatisfy({ self.sessions.synchronizationItemId(for: $0) == nil }) else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        var drafts = [LocalDraft]()
        drafts.reserveCapacity(selectedDrafts.count)
        for draft in selectedDrafts {
            drafts.append(
                try await reconciliation.synchronizedDraftForReconciliation(
                    itemId: draft.targetId ?? draft.id,
                    draft: draft
                )
            )
        }
        try context.ensureAuthority(authority)
        guard let primary = drafts.first else { return }
        guard drafts.allSatisfy({ $0.serverId != nil }) else {
            throw ReviewRequestError.draftNotSynchronized
        }
        let reconciliationByDraftId = Dictionary(
            reconciliations.map { ($0.candidate.draftId, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        guard drafts.allSatisfy({ draft in
            draft.freshness == .current
                || draft.serverId.flatMap { reconciliationByDraftId[$0] } != nil
        }) else {
            throw ReviewRequestError.reconciliationRequired
        }
        let selectedDraftIds = Set(drafts.compactMap(\.serverId))
        guard reconciliationByDraftId.keys.allSatisfy(selectedDraftIds.contains) else {
            throw ReviewRequestError.reconciliationRequired
        }
        let detail: ReviewDetail = try await context.server.send(
            method: "POST",
            path: "/api/v1/reviews",
            headers: [
                "If-Match": DraftReconciliationService.refETag(
                    reconciliations.first?.candidate.currentCommitId ?? primary.currentCommitId
                )
            ],
            body: CreateReviewRequest(
                drafts: drafts.map { draft in
                    let reconciliation = reconciliationByDraftId[draft.serverId!]
                    return ReviewDraftRequest(
                        draftId: draft.serverId!,
                        expectedDraftVersion: reconciliation?.candidate.draftVersion
                            ?? draft.serverVersion,
                        candidateId: reconciliation?.candidate.candidateId,
                        resolvedState: reconciliation?.resolvedState
                    )
                },
                title: title,
                description: description
            )
        )
        try context.ensureAuthority(authority)
        let record = WorkspaceLoader.mapReview(detail)
        reviews.insert(record, at: 0)
        selectedReviewId = record.id
        navigation.selectedSection = .reviews
    }

    func resubmit(
        _ review: ReviewRecord,
        detail: ReviewDetail,
        candidate: DraftReconciliationCandidate? = nil,
        resolvedState: ReconciliationResourceState? = nil
    ) async throws {
        let authority = context.authorityGeneration
        guard Self.isOrganizationDraft(detail.draft) else {
            throw ReviewRequestError.legacyProjectDraftCannotBePublished
        }
        guard context.isReviewAuthor(review) else {
            throw ServerClientError.forbidden("Only the draft author can resubmit this Review.")
        }
        guard detail.draft.coordination.freshness == .current || candidate != nil else {
            throw ReviewRequestError.reconciliationRequired
        }
        let updated: ReviewDetail = try await context.server.send(
            method: "POST",
            path: "/api/v1/reviews/\(review.id)/submissions",
            headers: [
                "If-Match": DraftReconciliationService.refETag(
                    candidate?.currentCommitId ?? detail.draft.coordination.currentCommitId
                )
            ],
            body: CreateReviewSubmissionRequest(
                expectedReviewVersion: review.version,
                drafts: (detail.drafts ?? [
                    ReviewDraftDetail(draft: detail.draft, operations: detail.operations)
                ]).map { draftDetail in
                    let reconciliation = candidate.flatMap { candidate in
                        candidate.draftId == draftDetail.draft.draftId ? candidate : nil
                    }
                    return ReviewDraftRequest(
                        draftId: draftDetail.draft.draftId,
                        expectedDraftVersion: reconciliation?.draftVersion
                            ?? draftDetail.draft.version,
                        candidateId: reconciliation?.candidateId,
                        resolvedState: reconciliation == nil ? nil : resolvedState
                    )
                },
                title: review.title,
                description: review.description
            )
        )
        try context.ensureAuthority(authority)
        replaceReview(with: WorkspaceLoader.mapReview(updated))
    }

    func reviewDetail(_ reviewId: String) async throws -> ReviewDetail {
        let authority = context.authorityGeneration
        let detail: ReviewDetail = try await context.server.get("/api/v1/reviews/\(reviewId)")
        try context.ensureAuthority(authority)
        return detail
    }

    func addComment(
        _ body: String,
        to review: ReviewRecord,
        anchorPath: String? = nil,
        anchorLine: Int? = nil
    ) async throws {
        let authority = context.authorityGeneration
        let _: ReviewComment = try await context.server.send(
            method: "POST",
            path: "/api/v1/reviews/\(review.id)/comments",
            body: CreateReviewCommentRequest(
                body: body,
                expectedReviewVersion: review.version,
                anchorPath: anchorPath,
                anchorLine: anchorLine
            )
        )
        try context.ensureAuthority(authority)
        try await refreshReview(review.id)
    }

    func decide(_ review: ReviewRecord, decision: String, note: String) async throws {
        let authority = context.authorityGeneration
        let detail: ReviewDetail = try await context.server.send(
            method: "POST",
            path: "/api/v1/reviews/\(review.id)/decisions",
            body: CreateReviewDecisionRequest(
                decision: decision,
                expectedReviewVersion: review.version,
                body: note
            )
        )
        try context.ensureAuthority(authority)
        replaceReview(with: WorkspaceLoader.mapReview(detail))
    }

    func merge(_ review: ReviewRecord) async throws {
        let authority = context.authorityGeneration
        guard context.canMergeReviews else {
            throw ServerClientError.forbidden("Your account cannot merge Reviews.")
        }
        // A Review belongs to its carrying Project, while its Draft targets
        // Organization authority. The coordination commit is the exact Org Ref
        // generation; the carrying Project's projection ETag is not that base.
        let detail = try await reviewDetail(review.id)
        try context.ensureAuthority(authority)
        let currentReview = WorkspaceLoader.mapReview(detail)
        replaceReview(with: currentReview)
        guard ReviewDecisionReadiness(review: review).matches(currentReview) else {
            throw ReviewRequestError.reviewChanged
        }
        guard Self.isOrganizationDraft(detail.draft) else {
            throw ReviewRequestError.legacyProjectDraftCannotBePublished
        }
        let _: ReviewMergeResponse = try await context.server.send(
            method: "POST",
            path: "/api/v1/reviews/\(review.id)/merges",
            headers: ["If-Match": DraftReconciliationService.refETag(detail.draft.coordination.currentCommitId)],
            body: CreateReviewMergeRequest(expectedReviewVersion: review.version)
        )
        try context.ensureAuthority(authority)
        await onMerged?()
    }

    nonisolated static func canRequestReview(_ draft: LocalDraft) -> Bool {
        draft.scope == .org
    }

    nonisolated static func reviewableProjectDrafts(
        _ drafts: [LocalDraft],
        projectId: String?
    ) -> [LocalDraft] {
        MemoryTreeProjection.preferredMemoryTreeDrafts(
            MemoryTreeProjection.memoryTreeDrafts(drafts, activeProjectId: projectId)
        ).filter {
            $0.status == .open && self.canRequestReview($0)
        }
    }

    private nonisolated static func isOrganizationDraft(_ draft: ServerDraft) -> Bool {
        draft.resource.scope == MemoryScope.org.rawValue
    }

    private func refreshReview(_ reviewId: String) async throws {
        let detail = try await reviewDetail(reviewId)
        replaceReview(with: WorkspaceLoader.mapReview(detail))
    }

    func replaceReview(with review: ReviewRecord) {
        if let index = reviews.firstIndex(where: { $0.id == review.id }) {
            reviews[index] = review
        } else {
            reviews.insert(review, at: 0)
        }
    }

    func startLoading(
        generation: UUID,
        requiresFreshData: Bool,
        baseSnapshotWasStale: Bool
    ) {
        cancelLoading()
        let loader = context.loader
        let baselineReviews = reviews
        reviewLoadState = .loading
        reviewLoadTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.context.workspaceReloadGeneration == generation {
                    self.reviewLoadTask = nil
                }
            }
            do {
                let loaded = try await loader.loadReviews()
                try Task.checkCancellation()
                guard let self,
                      context.workspaceReloadGeneration == generation,
                      context.phase == .ready else {
                    return
                }
                guard WorkspaceLoadPolicy.canPublishDeferredLoad(
                    requiresFreshData: requiresFreshData,
                    baseSnapshotWasStale: baseSnapshotWasStale,
                    responseWasStale: loaded.hasStaleServerResponse
                ) else {
                    reviewLoadState = .failed(
                        "Fresh Review data was unavailable. Existing Reviews were kept."
                    )
                    return
                }
                reviews = WorkspaceLoadPolicy.mergeDeferredRecords(
                    baseline: baselineReviews,
                    current: reviews,
                    loaded: loaded.records
                )
                reviewLoadState = .loaded
                if let selectedReviewId = selectedReviewId,
                   !self.reviews.contains(where: { $0.id == selectedReviewId }) {
                    self.selectedReviewId = nil
                }
            } catch is CancellationError {
                return
            } catch {
                guard let self, context.workspaceReloadGeneration == generation else { return }
                reviewLoadState = .failed(error.localizedDescription)
            }
        }
    }

    func cancelLoading() {
        reviewLoadTask?.cancel()
        reviewLoadTask = nil
    }

    func resetAuthority() {
        cancelLoading()
        reviews.removeAll()
        reviewLoadState = .loading
        selectedReviewId = nil
        pendingReviewReconciliationId = nil
        reviewDecisionReadiness = nil
    }

    func retainAccessibleProjects(_ projectIds: Set<String>) {
        reviews = WorkspaceLoadPolicy.retainingAccessibleProjectRecords(
            reviews, accessibleProjectIds: projectIds, projectId: \.projectId
        )
        if let selectedReviewId, !reviews.contains(where: { $0.id == selectedReviewId }) {
            self.selectedReviewId = nil
            reviewDecisionReadiness = nil
            pendingReviewReconciliationId = nil
        }
    }
}

enum ReviewMenuAction: Sendable, Equatable {
    case approve
    case reject
    case merge
    case resubmit

    func isAvailable(
        for review: ReviewRecord,
        canDecideReviews: Bool,
        canMergeReviews: Bool,
        isAuthor: Bool
    ) -> Bool {
        switch self {
        case .approve:
            return review.status == "open" && canDecideReviews && canMergeReviews
        case .reject:
            return review.status == "open" && canDecideReviews
        case .merge:
            return review.status == "approved"
                && review.approvedResultHash?.isEmpty == false
                && canMergeReviews
        case .resubmit:
            return review.status == "rejected" && isAuthor
        }
    }
}

struct ReviewDecisionReadiness: Equatable, Sendable {
    let reviewId: String
    let reviewVersion: Int
    let status: String
    let approvedResultHash: String?
    let freshness: DraftFreshness
    let reconciliation: DraftReconciliationStatus
    let currentCommitId: String?

    init(review: ReviewRecord) {
        reviewId = review.id
        reviewVersion = review.version
        status = review.status
        approvedResultHash = review.approvedResultHash
        freshness = review.freshness
        reconciliation = review.reconciliation
        currentCommitId = review.currentCommitId
    }

    func matches(_ review: ReviewRecord) -> Bool {
        self == ReviewDecisionReadiness(review: review)
    }
}
