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
    @Published private(set) var updates: [String: ReviewUpdateModel] = [:]

    @discardableResult
    func beginUpdate(_ review: ReviewRecord) -> ReviewUpdateModel? {
        if let existing = updates[review.id] { return existing }
        guard context.isReviewAuthor(review) || context.canMergeReview(review), review.freshness == .behind,
              ["open", "approved", "rejected"].contains(review.status) else { return nil }
        if reviewDecisionReadiness?.reviewId == review.id { reviewDecisionReadiness = nil }
        let update = ReviewUpdateModel(review: review, canResolveConflicts: context.isReviewAuthor(review), prepare: { [weak self] in
            guard let self else { throw CancellationError() }
            let authority = self.context.authorityGeneration
            let latest: ReviewDetail = try await self.context.server.get("/api/v1/reviews/\(review.id)")
            guard self.context.authorityGeneration == authority, !Task.isCancelled else {
                throw CancellationError()
            }
            guard latest.review.coordination.freshness == .behind else {
                return ReviewUpdatePlan(detail: latest, candidates: [])
            }
            return try await self.reconciliation.autoRebaseReview(latest)
        }, apply: { [weak self] plan, request in
            guard let self else { throw CancellationError() }
            return try await self.reconciliation.applyReviewUpdate(
                reviewId: review.id, plan: plan, request: request
            )
        })
        update.didLoad = { [weak self, weak update] in
            guard let self, let update, self.updates[review.id] === update else { return }
            if let plan = update.plan, update.errorMessage == nil {
                self.replaceReview(with: WorkspaceLoader.mapReview(plan.detail.review))
                if plan.candidates.isEmpty { self.endUpdate(review.id, result: plan.detail) }
            } else {
                self.objectWillChange.send()
            }
        }
        updates[review.id] = update
        return update
    }

    /// Shares preparation between queue rows and detail; only server-saved results become completed badges.
    func prepareUpdate(_ review: ReviewRecord) async {
        guard let update = beginUpdate(review) else { return }
        await update.load()
    }

    func canSaveConflictResolutions(_ review: ReviewRecord) -> Bool {
        guard context.isReviewAuthor(review), let update = updates[review.id] else { return false }
        return update.candidates.contains { $0.status == .conflicts }
    }

    func endUpdate(_ reviewId: String, result: ReviewDetail? = nil) {
        updates.removeValue(forKey: reviewId)?.invalidate()
        if let result { replaceReview(with: WorkspaceLoader.mapReview(result.review)) }
    }

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
                feedback.errorMessage = String(localized: "The Review for this draft is unavailable. It may have already been closed.")
                return
            }
            openReview(review)
        } catch {
            guard context.workspaceReloadGeneration == generation,
                  context.activeProjectId == projectId,
                  navigation.selectedSection == section, !Task.isCancelled else { return }
            feedback.errorMessage = error.actionMessage
        }
    }

    var submittedProjectDrafts: [LocalDraft] {
        guard let activeProjectId = context.activeProjectId else { return [] }
        return edits.drafts.filter { $0.projectId == activeProjectId && $0.status == .submitted }
    }

    func canPerformReviewMenuAction(_ action: ReviewMenuAction) -> Bool {
        guard selectedReviewId.flatMap({ updates[$0] }) == nil, context.phase == .ready,
              navigation.selectedSection == .reviews,
              let selectedReviewId = selectedReviewId,
              let review = reviews.first(where: { $0.id == selectedReviewId }),
              reviewDecisionReadiness?.matches(review) == true,
              review.freshness == .current else {
            return false
        }
        return action.isAvailable(
            for: review,
            canDecideReviews: context.canDecideReview(review),
            canMergeReviews: context.canMergeReview(review),
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
            feedback.errorMessage = error.actionMessage
        }
    }

    func requestReview(
        for draft: LocalDraft,
        title: String,
        description: String,
        candidate: DraftReconciliationCandidate? = nil,
        resolvedState: ReconciliationResourceState? = nil,
        contributions: [OrgContributionEntry] = []
    ) async throws {
        let authority = context.authorityGeneration
        if sessions.synchronizationItemId(for: draft) != nil {
            throw DocumentSyncError.mutationWhileSynchronizing
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
                orgContribution: try Self.contributionRequest(contributions, drafts: [draft]),
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
        reconciliations: [ReviewDraftReconciliation] = [],
        contributions: [OrgContributionEntry] = []
    ) async throws {
        let authority = context.authorityGeneration
        let selectedDrafts = drafts.sorted {
            $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending
        }
        guard let selectedPrimary = selectedDrafts.first else { return }
        guard selectedDrafts.allSatisfy({ $0.projectId == selectedPrimary.projectId }) else {
            throw ReviewRequestError.mixedProjects
        }
        guard selectedDrafts.allSatisfy({ $0.scope == selectedPrimary.scope }) else {
            throw ReviewRequestError.mixedScopes
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
                orgContribution: try Self.contributionRequest(contributions, drafts: drafts),
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

    private static func contributionRequest(_ entries: [OrgContributionEntry], drafts: [LocalDraft]) throws -> [OrgContributionEntry]? {
        guard !entries.isEmpty else { return nil }
        return try entries.map { entry in
            guard let draft = drafts.first(where: { $0.id == entry.draftId }), draft.scope == .project,
                  let serverId = draft.serverId else { throw ReviewRequestError.draftNotSynchronized }
            return .init(draftId: serverId, targetId: entry.targetId, path: entry.path)
        }
    }

    func retryOrgContribution(_ review: ReviewRecord) async throws {
        let authority = context.authorityGeneration
        let detail: ReviewDetail = try await context.server.send(method: "POST",
            path: "/api/v1/reviews/\(review.id)/org-contribution", body: [String: String]())
        try context.ensureAuthority(authority)
        replaceReview(with: WorkspaceLoader.mapReview(detail))
    }

    func resubmit(
        _ review: ReviewRecord,
        detail: ReviewDetail,
        candidate: DraftReconciliationCandidate? = nil,
        resolvedState: ReconciliationResourceState? = nil
    ) async throws {
        let authority = context.authorityGeneration
        guard context.isReviewAuthor(review) else {
            throw ServerClientError.forbidden(String(localized: "Only the draft author can resubmit this Review."))
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
        guard context.canMergeReview(review) else {
            throw ServerClientError.forbidden(String(localized: "Your account cannot merge Reviews."))
        }
        // The coordination commit belongs to the Review's publication owner.
        // Org proposals use the Org Ref; Project proposals use the Project Ref.
        let detail = try await reviewDetail(review.id)
        try context.ensureAuthority(authority)
        let currentReview = WorkspaceLoader.mapReview(detail)
        replaceReview(with: currentReview)
        guard ReviewDecisionReadiness(review: review).matches(currentReview) else {
            throw ReviewRequestError.reviewChanged
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
        draft.status == .open
    }

    nonisolated static func reviewableProjectDrafts(
        _ drafts: [LocalDraft],
        projectId: String?
    ) -> [LocalDraft] {
        MemoryTreeProjection.preferredMemoryTreeDrafts(
            MemoryTreeProjection.memoryTreeDrafts(drafts, activeProjectId: projectId)
        ).filter {
            $0.scope == .project && $0.status == .open && self.canRequestReview($0)
        }
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
                        String(localized: "Fresh Review data was unavailable. Existing Reviews were kept.")
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
            } catch where error.isUserCancellation {
                return
            } catch {
                guard let self, context.workspaceReloadGeneration == generation else { return }
                reviewLoadState = .failed(error.userFacingMessage)
            }
        }
    }

    func cancelLoading() {
        reviewLoadTask?.cancel()
        reviewLoadTask = nil
    }

    func resetAuthority() {
        for id in Array(updates.keys) { endUpdate(id) }
        cancelLoading()
        reviews.removeAll()
        reviewLoadState = .loading
        selectedReviewId = nil
        pendingReviewReconciliationId = nil
        reviewDecisionReadiness = nil
    }

    func retainAccessibleProjects(_ projectIds: Set<String>) {
        for (id, update) in updates where !projectIds.contains(update.review.projectId) { endUpdate(id) }
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
