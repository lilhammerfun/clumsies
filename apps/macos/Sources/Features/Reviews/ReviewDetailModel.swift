import Combine
import Foundation

enum ReviewCommentTarget: Hashable {
    case general
    case line(Int)
}

struct ReviewCommentPlacement: Equatable {
    let general: [ReviewComment]
    let byLine: [Int: [ReviewComment]]
    let unplaced: [ReviewComment]

    static func resolve(
        comments: [ReviewComment],
        activePath: String?,
        renderableLines: Set<Int>,
        minimumInlineVersion: Int
    ) -> ReviewCommentPlacement {
        var general: [ReviewComment] = []
        var byLine: [Int: [ReviewComment]] = [:]
        var unplaced: [ReviewComment] = []

        for comment in comments {
            switch (comment.anchorPath, comment.anchorLine) {
            case (nil, nil):
                general.append(comment)
            case let (path?, line?)
                where path == activePath
                    && renderableLines.contains(line)
                    && comment.reviewVersion >= minimumInlineVersion:
                byLine[line, default: []].append(comment)
            default:
                unplaced.append(comment)
            }
        }

        return .init(general: general, byLine: byLine, unplaced: unplaced)
    }

    static func minimumInlineVersion(reviewVersion: Int, status: String) -> Int {
        let lifecycleVersionsAfterContent: Int
        switch status {
        case "approved", "rejected":
            lifecycleVersionsAfterContent = 1
        case "merged":
            lifecycleVersionsAfterContent = 2
        default:
            lifecycleVersionsAfterContent = 0
        }
        return max(1, reviewVersion - lifecycleVersionsAfterContent)
    }
}

struct ReviewFileDescriptor: Identifiable, Hashable, Sendable {
    let id: String
    let draftId: String
    let path: String
    let needsUpdate: Bool
    let hasConflicts: Bool

    static func resolve(
        reviewId: String,
        detail: ReviewDraftDetail,
        loadedPath: String? = nil
    ) -> ReviewFileDescriptor {
        let initialPath = detail.operations.first?.resource.path ?? detail.draft.resource.path
        let proposedPath = detail.operations.reduce(initialPath) { path, operation in
            if let newPath = operation.newPath { return newPath }
            if operation.action == "create", let createdPath = operation.resource.path { return createdPath }
            return path
        }
        let path = loadedPath ?? proposedPath ?? detail.draft.resource.id ?? "Untitled"
        let id = detail.draft.resource.id ?? "review-file:\(reviewId):\(detail.draft.draftId)"
        let needsUpdate = ["open", "submitted"].contains(detail.draft.status)
            && detail.draft.coordination.freshness == .behind
        return .init(
            id: id,
            draftId: detail.draft.draftId,
            path: path,
            needsUpdate: needsUpdate,
            hasConflicts: needsUpdate && detail.draft.coordination.reconciliation == .conflicts
        )
    }
}

@MainActor
final class ReviewDetailModel: ObservableObject {
    private let fetchDetail: (String) async throws -> ReviewDetail
    private var authorityObservation: AnyCancellable?
    let reviewId: String
    private let workspaceContext: WorkspaceContext
    private let workspaceFeedback: WorkspaceFeedback
    private let reviewModel: ReviewsModel

    init(reviewId: String, context: WorkspaceContext, feedback: WorkspaceFeedback,
         reviews: ReviewsModel,
         fetchDetail: ((String) async throws -> ReviewDetail)? = nil) {
        self.reviewId = reviewId
        workspaceContext = context
        workspaceFeedback = feedback
        reviewModel = reviews
        self.fetchDetail = fetchDetail ?? { try await reviews.reviewDetail($0) }
        authorityObservation = context.$authorityGeneration.dropFirst().sink { [weak self] _ in
            self?.invalidateDetailRequests()
            self?.detail = nil
            self?.loadedPaths = [:]
            self?.loading = false
            self?.loadError = nil
        }
    }

    @Published var detail: ReviewDetail?
    private var fileLoader: ReviewFileLoader?
    private var fileLoadTask: Task<Void, Never>?
    @Published var loadedPaths: [String: String] = [:]
    @Published var loadingFile = false
    @Published var fileLoadError: String?
    @Published var changeSources: ReviewChangeSources?
    @Published var diffModel: SplitDiffModel?
    @Published var loading = true
    @Published var loadError: String?
    @Published var composing: ReviewCommentTarget?
    @Published var commentDraft = ""
    @Published var isSubmittingComment = false
    @Published var selectedFileId: String?
    @Published var showsGeneralComments = false
    private var detailRequestGeneration = UUID()

    private struct DetailRequest {
        let generation: UUID
        let baseline: ReviewDecisionReadiness?
    }

    var review: ReviewRecord? {
        let loadedReview = detail.map { WorkspaceLoader.mapReview($0.review) }
        let storedReview = reviewModel.reviews.first { $0.id == self.reviewId }
        if let loadedReview, let storedReview {
            return storedReview.version >= loadedReview.version ? storedReview : loadedReview
        }
        return storedReview ?? loadedReview
    }

    var storedReviewDecisionSignature: ReviewDecisionReadiness? {
        reviewModel.reviews.first { $0.id == self.reviewId }.map(ReviewDecisionReadiness.init)
    }

    var draftDetails: [ReviewDraftDetail] {
        guard let detail else { return [] }
        return detail.drafts ?? [ReviewDraftDetail(draft: detail.draft, operations: detail.operations)]
    }

    var fileDescriptors: [ReviewFileDescriptor] {
        draftDetails.map {
            ReviewFileDescriptor.resolve(
                reviewId: self.reviewId,
                detail: $0,
                loadedPath: self.loadedPaths[$0.draft.draftId]
            )
        }
    }

    var selectedDraftDetail: ReviewDraftDetail? {
        guard let selectedFileId else { return draftDetails.first }
        return draftDetails.first {
            ReviewFileDescriptor.resolve(reviewId: self.reviewId, detail: $0).id == selectedFileId
        }
    }

    var commentPlacement: ReviewCommentPlacement {
        let loadedReview = detail?.review
        return ReviewCommentPlacement.resolve(
            comments: detail?.comments ?? [],
            activePath: changeSources?.proposedPath,
            renderableLines: Set(diffModel?.rows.compactMap { $0.modified?.lineNumber } ?? []),
            minimumInlineVersion: ReviewCommentPlacement.minimumInlineVersion(
                reviewVersion: loadedReview?.version ?? 1,
                status: loadedReview?.status ?? "open"
            )
        )
    }

    var generalComments: [ReviewComment] {
        commentPlacement.general
    }

    var commentsByLine: [Int: [ReviewComment]] {
        commentPlacement.byLine
    }

    var unplacedComments: [ReviewComment] {
        commentPlacement.unplaced
    }

    var reviewWideCommentCount: Int {
        generalComments.count + unplacedComments.count
    }

    func toggleGeneralComments() {
        if showsGeneralComments {
            showsGeneralComments = false
            return
        }

        showsGeneralComments = true
        if reviewWideCommentCount == 0 {
            composing = .general
            commentDraft = ""
        }
    }

    var composingLine: Int? {
        if case .line(let line) = composing { return line }
        return nil
    }

    func load() async {
        let request = beginDetailRequest()
        loading = true
        loadError = nil
        detail = nil
        loadedPaths = [:]
        changeSources = nil
        diffModel = nil
        composing = nil
        commentDraft = ""
        selectedFileId = nil
        showsGeneralComments = false
        defer {
            if detailRequestGeneration == request.generation {
                loading = false
            }
        }
        do {
            let loadedDetail = try await fetchDetail(reviewId)
            applyLoadedDetail(
                loadedDetail,
                request: request
            )
        } catch {
            guard !Task.isCancelled,
                  detailRequestGeneration == request.generation else { return }
            clearDecisionReadiness()
            loadError = error.localizedDescription
            workspaceFeedback.errorMessage = error.localizedDescription
        }
    }

    func refreshDetail() async {
        let request = beginDetailRequest()
        do {
            let loadedDetail = try await fetchDetail(reviewId)
            applyLoadedDetail(
                loadedDetail,
                request: request
            )
        } catch {
            guard !Task.isCancelled,
                  detailRequestGeneration == request.generation else { return }
            clearDecisionReadiness()
            if detail == nil {
                loading = false
                loadError = error.localizedDescription
            }
            workspaceFeedback.errorMessage = error.localizedDescription
        }
    }

    private func beginDetailRequest() -> DetailRequest {
        invalidateDetailRequests()
        return DetailRequest(
            generation: detailRequestGeneration,
            baseline: storedReviewDecisionSignature
        )
    }

    func invalidateDetailRequests() {
        detailRequestGeneration = UUID()
        isSubmittingComment = false
        fileLoadTask?.cancel()
        fileLoadTask = nil
        if let fileLoader { Task { await fileLoader.cancel() } }
        fileLoader = nil
        changeSources = nil
        diffModel = nil
        fileLoadError = nil
        loadingFile = false
        clearDecisionReadiness()
    }

    func clearDecisionReadiness() {
        if reviewModel.reviewDecisionReadiness?.reviewId == reviewId {
            reviewModel.reviewDecisionReadiness = nil
        }
    }

    private func applyLoadedDetail(
        _ loadedDetail: ReviewDetail,
        request: DetailRequest
    ) {
        guard !Task.isCancelled,
              detailRequestGeneration == request.generation,
              storedReviewDecisionSignature == request.baseline else { return }
        let loadedReview = WorkspaceLoader.mapReview(loadedDetail.review)
        if let baseline = request.baseline,
           loadedReview.version < baseline.reviewVersion {
            loading = false
            loadError = "The Review changed while its detail was loading. Try again."
            return
        }

        detail = loadedDetail
        loadedPaths = [:]
        let client = workspaceContext.server
        fileLoader = ReviewFileLoader { id in
            try await client.get("/api/v1/commits/\(id)")
        }
        loading = false
        loadError = nil
        ClientDiagnostics.record("review_directory_loaded", ["file_count": String(draftDetails.count)])
        let availableIds = Set(fileDescriptors.map(\.id))
        if selectedFileId == nil || !availableIds.contains(selectedFileId!) {
            selectedFileId = fileDescriptors.first?.id
        } else {
            selectCurrentFile()
        }
        reviewModel.replaceReview(with: loadedReview)
        reviewModel.beginUpdate(loadedReview)
    }

    func selectCurrentFile() {
        fileLoadTask?.cancel()
        changeSources = nil
        diffModel = nil
        fileLoadError = nil
        composing = nil
        commentDraft = ""
        clearDecisionReadiness()
        guard let selectedDraftDetail, let fileLoader else { return }
        let generation = detailRequestGeneration
        let fileId = selectedFileId
        loadingFile = true
        fileLoadTask = Task {
            let started = ContinuousClock.now
            do {
                let content = try await fileLoader.load(selectedDraftDetail)
                guard !Task.isCancelled, self.detailRequestGeneration == generation,
                      self.selectedFileId == fileId else { return }
                self.changeSources = content.sources
                self.diffModel = content.diff
                self.loadedPaths[selectedDraftDetail.draft.draftId] = content.sources.proposedPath
                self.loadingFile = false
                self.markCurrentDetailDecisionReady()
                let elapsed = started.duration(to: .now).components
                ClientDiagnostics.record("review_file_loaded", [
                    "elapsed_ms": String(elapsed.seconds * 1_000 + elapsed.attoseconds / 1_000_000_000_000_000)
                ])
            } catch {
                guard !Task.isCancelled, self.detailRequestGeneration == generation,
                      self.selectedFileId == fileId else { return }
                self.loadingFile = false
                self.fileLoadError = error.localizedDescription
                ClientDiagnostics.record("review_file_load_failed", ClientDiagnostics.failureFields(error))
            }
        }
    }

    func markCurrentDetailDecisionReady() {
        guard let detail, changeSources != nil, !loadingFile, fileLoadError == nil,
              reviewModel.updates[reviewId] == nil else {
            clearDecisionReadiness()
            return
        }
        let loadedReview = WorkspaceLoader.mapReview(detail.review)
        guard storedReviewDecisionSignature == ReviewDecisionReadiness(review: loadedReview) else {
            clearDecisionReadiness()
            return
        }
        reviewModel.reviewDecisionReadiness = ReviewDecisionReadiness(review: loadedReview)
    }

    func submitComment(line: Int?) async {
        guard !isSubmittingComment, let detail,
              !commentDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return
        }
        let renderedReview = WorkspaceLoader.mapReview(detail.review)
        let anchorPath = line == nil ? nil : changeSources?.proposedPath
        guard line == nil || anchorPath != nil else {
            workspaceFeedback.errorMessage = "The proposed file path is unavailable for this line comment."
            return
        }
        let generation = detailRequestGeneration
        isSubmittingComment = true
        defer { if detailRequestGeneration == generation { isSubmittingComment = false } }
        do {
            try await reviewModel.addComment(
                commentDraft,
                to: renderedReview,
                anchorPath: anchorPath,
                anchorLine: line
            )
            guard detailRequestGeneration == generation, !Task.isCancelled else { return }
            composing = nil
            commentDraft = ""
            await refreshDetail()
        } catch {
            guard detailRequestGeneration == generation, !Task.isCancelled else { return }
            workspaceFeedback.errorMessage = error.localizedDescription
            if let serverError = error as? ServerClientError,
               case .response(let status, _) = serverError,
               status == 409 {
                await refreshDetail()
            }
        }
    }

    func handlePendingReconciliation(_ pendingReviewId: String?) {
        guard pendingReviewId == reviewId, let review else { return }
        reviewModel.pendingReviewReconciliationId = nil
        reviewModel.beginUpdate(review)
    }
}
