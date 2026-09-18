import SwiftUI

private enum ReviewCommentTarget: Hashable {
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
    let path: String

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
        return .init(id: id, path: path)
    }
}

struct ReviewDetailPage: View {
    @ObservedObject var store: WorkspaceStore
    let reviewId: String
    let loadsRemoteContent: Bool

    @State private var detail: ReviewDetail?
    @State private var fileLoader: ReviewFileLoader?
    @State private var fileLoadTask: Task<Void, Never>?
    @State private var loadedPaths: [String: String] = [:]
    @State private var loadingFile = false
    @State private var fileLoadError: String?
    @State private var changeSources: ReviewChangeSources?
    @State private var diffModel: SplitDiffModel?
    @State private var loading = true
    @State private var loadError: String?
    @State private var composing: ReviewCommentTarget?
    @State private var commentDraft = ""
    @State private var isSubmittingComment = false
    @State private var reconciliationCandidate: DraftReconciliationCandidate?
    @State private var loadsReconciliation = false
    @State private var selectedFileId: String?
    @State private var showsGeneralComments = false
    @State private var detailRequestGeneration = UUID()

    private struct DetailRequest {
        let generation: UUID
        let baseline: ReviewDecisionReadiness?
    }

    private var review: ReviewRecord? {
        let loadedReview = detail.map { WorkspaceLoader.mapReview($0.review) }
        let storedReview = store.reviews.first { $0.id == reviewId }
        if let loadedReview, let storedReview {
            return storedReview.version >= loadedReview.version ? storedReview : loadedReview
        }
        return storedReview ?? loadedReview
    }

    private var storedReviewDecisionSignature: ReviewDecisionReadiness? {
        store.reviews.first { $0.id == reviewId }.map(ReviewDecisionReadiness.init)
    }

    private var draftDetails: [ReviewDraftDetail] {
        guard let detail else { return [] }
        return detail.drafts ?? [ReviewDraftDetail(draft: detail.draft, operations: detail.operations)]
    }

    private var fileDescriptors: [ReviewFileDescriptor] {
        draftDetails.map {
            ReviewFileDescriptor.resolve(
                reviewId: reviewId,
                detail: $0,
                loadedPath: loadedPaths[$0.draft.draftId]
            )
        }
    }

    private var selectedDraftDetail: ReviewDraftDetail? {
        guard let selectedFileId else { return draftDetails.first }
        return draftDetails.first {
            ReviewFileDescriptor.resolve(reviewId: reviewId, detail: $0).id == selectedFileId
        }
    }

    private var commentPlacement: ReviewCommentPlacement {
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

    private var generalComments: [ReviewComment] {
        commentPlacement.general
    }

    private var commentsByLine: [Int: [ReviewComment]] {
        commentPlacement.byLine
    }

    private var unplacedComments: [ReviewComment] {
        commentPlacement.unplaced
    }

    var body: some View {
        Group {
            if let candidate = reconciliationCandidate {
                DraftReconciliationView(
                    candidate: candidate,
                    onCancel: {
                        reconciliationCandidate = nil
                        markCurrentDetailDecisionReady()
                    },
                    onApplied: {
                        reconciliationCandidate = nil
                        Task { await refreshDetail() }
                    }
                ) { resolvedState in
                    try await store.applyReconciliation(
                        draftId: candidate.draftId,
                        candidate: candidate,
                        resolvedState: resolvedState,
                        projectId: draftDetails.first {
                            $0.draft.draftId == candidate.draftId
                        }?.draft.projectId
                    )
                }
            } else if loading {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let loadError {
                ContentUnavailableView {
                    Label("Unable to Load Review", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(loadError)
                } actions: {
                    Button("Try Again") {
                        Task { await load() }
                    }
                }
            } else if let review, detail != nil, !draftDetails.isEmpty {
                content(review)
            } else {
                ContentUnavailableView(
                    "Review Unavailable",
                    systemImage: "checkmark.bubble",
                    description: Text("This Review is no longer in the workspace.")
                )
            }
        }
        .task(id: reviewId) {
            guard loadsRemoteContent else {
                loading = false
                return
            }
            await load()
        }
        .onDisappear {
            invalidateDetailRequests()
        }
        .navigationTitle(review?.title ?? "Review")
        .onChange(of: store.pendingReviewReconciliationId) { _, reviewId in
            handlePendingReconciliation(reviewId)
        }
        .onChange(of: selectedFileId) { _, _ in
            selectCurrentFile()
        }
        .onChange(of: storedReviewDecisionSignature) { _, signature in
            guard let signature,
                  detail.map({ ReviewDecisionReadiness(review: WorkspaceLoader.mapReview($0.review)) })
                    != signature else { return }
            invalidateDetailRequests()
            Task { await refreshDetail() }
        }
    }

    private func content(_ review: ReviewRecord) -> some View {
        return HSplitView {
            ReviewFileNavigator(
                files: fileDescriptors,
                selection: $selectedFileId
            )
            .frame(minWidth: 180, idealWidth: 220, maxWidth: 280)

            Group {
                if let selectedDraftDetail {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 20) {
                            reviewHeader(review)

                            if review.freshness == .behind {
                                readinessChip(
                                    review,
                                    detail: selectedDraftDetail
                                )
                            }

                            if showsGeneralComments {
                                generalCommentsPanel
                            }

                            diffPanel(detail: selectedDraftDetail)
                        }
                        .frame(maxWidth: 1180, alignment: .leading)
                        .frame(maxWidth: .infinity, alignment: .top)
                        .padding(.horizontal, 24)
                        .padding(.top, 24)
                        .padding(.bottom, 48)
                    }
                } else {
                    ContentUnavailableView(
                        "Select a File",
                        systemImage: "doc.text",
                        description: Text("Choose a changed file from the file navigator.")
                    )
                }
            }
            .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(nsColor: .windowBackgroundColor))
        }
        .onAppear {
            if selectedFileId == nil {
                selectedFileId = fileDescriptors.first?.id
            }
        }
    }

    private func reviewHeader(_ review: ReviewRecord) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(review.title)
                    .font(.title2.weight(.semibold))
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)

                Spacer(minLength: 16)

                if review.status == "merged", let decider = review.decidedBy {
                    HStack(spacing: 5) {
                        ReviewStatusIndicator(status: review.status)
                        Text("by")
                            .foregroundStyle(.secondary)
                        UserIdentityLabel(
                            account: decider,
                            displayName: decider.displayName ?? decider.email
                        )
                    }
                    .font(.caption)
                    .help(
                        TimestampFormatting.absoluteText(review.decidedAt).map {
                            "Merged by \(decider.displayName ?? decider.email) at \($0)"
                        } ?? "Merged by \(decider.displayName ?? decider.email)"
                    )
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel("Merged by \(decider.displayName ?? decider.email)")
                } else {
                    ReviewStatusIndicator(status: review.status)
                }

                Button(action: toggleGeneralComments) {
                    Image(systemName: reviewWideCommentCount == 0 ? "bubble.badge.plus" : "bubble")
                }
                .buttonStyle(.borderless)
                .help(reviewWideCommentCount == 0
                    ? "Add a review-wide comment"
                    : "Show \(reviewWideCommentCount) review-wide comments")
                .accessibilityLabel(reviewWideCommentCount == 0
                    ? "Add a review-wide comment"
                    : "Show \(reviewWideCommentCount) review-wide comments")
            }

            metadata(review)

            let description = review.description.trimmingCharacters(in: .whitespacesAndNewlines)
            if !description.isEmpty {
                Text(description)
                    .foregroundStyle(.primary)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }

            if review.status != "open" {
                decisionSummary(review)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func metadata(_ review: ReviewRecord) -> some View {
        let author = review.author.displayName ?? review.author.email
        let project = store.projects.first { $0.id == review.projectId }?.name
        let context = [author, project]
            .compactMap { $0 }
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
        let updated = TimestampFormatting.relativeText(review.updatedAt, relativeTo: .now)
            .map { " · Updated \($0)" } ?? ""
        return Text("\(context)\(updated)")
        .font(.caption)
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .truncationMode(.tail)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func readinessChip(_ review: ReviewRecord, detail: ReviewDraftDetail) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Label(
                review.reconciliation == .conflicts
                    ? "Resolve conflicts before deciding"
                    : "The shared version changed",
                systemImage: review.reconciliation == .conflicts
                    ? "exclamationmark.triangle"
                    : "arrow.trianglehead.2.clockwise.rotate.90"
            )
            .foregroundStyle(review.reconciliation == .conflicts ? Color.orange : Color.secondary)

            Spacer(minLength: 8)

            Button("Review Changes…") {
                loadReconciliation(detail: detail)
            }
            .controlSize(.small)
        }
        .font(.caption)
        .fixedSize(horizontal: false, vertical: true)
        .help(
            review.reconciliation == .conflicts
                ? "Draft conflicts with the shared version; resolve before deciding"
                : "Review base is behind the shared version; review changes before deciding"
        )
    }

    private func decisionSummary(_ review: ReviewRecord) -> some View {
        let relativeDecisionTime = review.decidedAt.flatMap {
            TimestampFormatting.relativeText($0, relativeTo: .now)
        }
        return VStack(alignment: .leading, spacing: 7) {
            if review.status != "merged" {
                HStack(spacing: 7) {
                    Image(systemName: decisionSymbol(review.status))
                        .foregroundStyle(decisionColor(review.status))
                    Text(decisionTitle(review.status))
                        .font(.callout.weight(.semibold))
                    if let decider = review.decidedBy {
                        let deciderName = decider.displayName ?? decider.email
                        Text("· Decision by \(deciderName)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    if let decidedAt = review.decidedAt,
                       let relativeDecisionTime {
                        Text("· \(relativeDecisionTime)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .help(TimestampFormatting.absoluteText(decidedAt).map {
                                "Decision recorded at \($0)"
                            } ?? "Decision time")
                    }
                }
            }

            if let body = review.decisionBody?.trimmingCharacters(in: .whitespacesAndNewlines),
               !body.isEmpty {
                Text(body)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }

        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var generalCommentsPanel: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Review comments")
                    .font(.headline)

                Spacer()

                if composing != .general {
                    Button {
                        composing = .general
                        commentDraft = ""
                    } label: {
                        Image(systemName: "plus")
                    }
                    .buttonStyle(.borderless)
                    .help("Add a review-wide comment")
                    .accessibilityLabel("Add a review-wide comment")
                }
            }

            if composing == .general {
                ReviewCommentComposer(
                    text: $commentDraft,
                    isSubmitting: isSubmittingComment,
                    onCancel: { composing = nil; commentDraft = "" },
                    onSubmit: { Task { await submitComment(line: nil) } }
                )
            }

            ForEach(generalComments) { comment in
                ReviewCommentRow(comment: comment) {
                    composing = .general
                    commentDraft = ""
                }
            }

            if !unplacedComments.isEmpty {
                Text("Comments from an earlier revision or file path")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                ForEach(unplacedComments) { comment in
                    VStack(alignment: .leading, spacing: 4) {
                        if let path = comment.anchorPath, let line = comment.anchorLine {
                            Text("\(path):\(line)")
                                .font(.caption.monospaced())
                                .foregroundStyle(.secondary)
                                .textSelection(.enabled)
                        }
                        ReviewCommentRow(comment: comment) {
                            composing = .general
                            commentDraft = ""
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func diffPanel(detail: ReviewDraftDetail) -> some View {
        if loadingFile {
            ProgressView("Loading file changes…")
                .frame(maxWidth: .infinity)
                .padding(.vertical, 20)
        } else if let fileLoadError {
            VStack(alignment: .leading, spacing: 8) {
                Label("Unable to Load File", systemImage: "exclamationmark.triangle")
                Text(fileLoadError).foregroundStyle(.secondary).textSelection(.enabled)
                Button("Try Again") { selectCurrentFile() }
            }
            .padding(.vertical, 20)
        } else if detail.operations.last?.action == "delete" {
            Label {
                Text("This Review deletes the selected memory. There is no proposed file to render.")
            } icon: {
                Image(systemName: "trash")
            }
            .font(.callout)
            .foregroundStyle(.secondary)
            .padding(.vertical, 20)
        } else if let diffModel {
            UnifiedDiffView(
                model: diffModel,
                commentsByLine: commentsByLine,
                composingLine: composingLine,
                commentDraft: $commentDraft,
                isSubmittingComment: isSubmittingComment,
                onRequestComment: { composing = .line($0) },
                onCancelComment: { composing = nil; commentDraft = "" },
                onSubmitComment: { line in Task { await submitComment(line: line) } },
                onReply: { line in composing = .line(line) }
            )
        } else if changeSources != nil {
            Text("This Review changes metadata without changing text content.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .padding(.vertical, 20)
        }
    }

    private var reviewWideCommentCount: Int {
        generalComments.count + unplacedComments.count
    }

    private func toggleGeneralComments() {
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

    private var composingLine: Int? {
        if case .line(let line) = composing { return line }
        return nil
    }

    private func load() async {
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
            let loadedDetail = try await store.reviewDetail(reviewId)
            applyLoadedDetail(
                loadedDetail,
                request: request
            )
        } catch {
            guard !Task.isCancelled,
                  detailRequestGeneration == request.generation else { return }
            clearDecisionReadiness()
            loadError = error.localizedDescription
            store.errorMessage = error.localizedDescription
        }
    }

    private func refreshDetail() async {
        let request = beginDetailRequest()
        do {
            let loadedDetail = try await store.reviewDetail(reviewId)
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
            store.errorMessage = error.localizedDescription
        }
    }

    private func beginDetailRequest() -> DetailRequest {
        invalidateDetailRequests()
        return DetailRequest(
            generation: detailRequestGeneration,
            baseline: storedReviewDecisionSignature
        )
    }

    private func invalidateDetailRequests() {
        detailRequestGeneration = UUID()
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

    private func clearDecisionReadiness() {
        if store.reviewDecisionReadiness?.reviewId == reviewId {
            store.reviewDecisionReadiness = nil
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
        let client = store.server
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
        store.replaceReview(with: loadedReview)
    }

    private func selectCurrentFile() {
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
                guard !Task.isCancelled, detailRequestGeneration == generation,
                      selectedFileId == fileId else { return }
                changeSources = content.sources
                diffModel = content.diff
                loadedPaths[selectedDraftDetail.draft.draftId] = content.sources.proposedPath
                loadingFile = false
                markCurrentDetailDecisionReady()
                let elapsed = started.duration(to: .now).components
                ClientDiagnostics.record("review_file_loaded", [
                    "elapsed_ms": String(elapsed.seconds * 1_000 + elapsed.attoseconds / 1_000_000_000_000_000)
                ])
            } catch {
                guard !Task.isCancelled, detailRequestGeneration == generation,
                      selectedFileId == fileId else { return }
                loadingFile = false
                fileLoadError = error.localizedDescription
                ClientDiagnostics.record("review_file_load_failed", ClientDiagnostics.failureFields(error))
            }
        }
    }

    private func markCurrentDetailDecisionReady() {
        guard let detail, changeSources != nil, !loadingFile, fileLoadError == nil else {
            clearDecisionReadiness()
            return
        }
        let loadedReview = WorkspaceLoader.mapReview(detail.review)
        guard storedReviewDecisionSignature == ReviewDecisionReadiness(review: loadedReview) else {
            clearDecisionReadiness()
            return
        }
        store.reviewDecisionReadiness = ReviewDecisionReadiness(review: loadedReview)
    }

    private func submitComment(line: Int?) async {
        guard let detail,
              !commentDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return
        }
        let renderedReview = WorkspaceLoader.mapReview(detail.review)
        let anchorPath = line == nil ? nil : changeSources?.proposedPath
        guard line == nil || anchorPath != nil else {
            store.errorMessage = "The proposed file path is unavailable for this line comment."
            return
        }
        isSubmittingComment = true
        defer { isSubmittingComment = false }
        do {
            try await store.addComment(
                commentDraft,
                to: renderedReview,
                anchorPath: anchorPath,
                anchorLine: line
            )
            composing = nil
            commentDraft = ""
            await refreshDetail()
        } catch {
            store.errorMessage = error.localizedDescription
            if let serverError = error as? ServerClientError,
               case .response(let status, _) = serverError,
               status == 409 {
                await refreshDetail()
            }
        }
    }

    private func loadReconciliation(detail: ReviewDraftDetail?) {
        guard let detail, !loadsReconciliation else { return }
        clearDecisionReadiness()
        loadsReconciliation = true
        Task {
            defer { loadsReconciliation = false }
            do {
                reconciliationCandidate = try await store.reconciliationCandidate(for: detail)
            } catch {
                markCurrentDetailDecisionReady()
                store.errorMessage = error.localizedDescription
            }
        }
    }

    private func handlePendingReconciliation(_ reviewId: String?) {
        guard reviewId == self.reviewId, let detail = selectedDraftDetail else { return }
        store.pendingReviewReconciliationId = nil
        loadReconciliation(detail: detail)
    }

    private func decisionTitle(_ status: String) -> String {
        switch status {
        case "approved": "Approved"
        case "rejected": "Changes requested"
        case "merged": "Merged"
        default: status.capitalized
        }
    }

    private func decisionSymbol(_ status: String) -> String {
        switch status {
        case "approved": "checkmark.circle.fill"
        case "rejected": "xmark.circle.fill"
        case "merged": "arrow.triangle.merge"
        default: "circle.fill"
        }
    }

    private func decisionColor(_ status: String) -> Color {
        switch status {
        case "approved": .green
        case "rejected": .red
        default: .secondary
        }
    }

}

private struct ReviewFileNavigator: View {
    let files: [ReviewFileDescriptor]
    @Binding var selection: String?

    var body: some View {
        PathTreeView(
            items: files.map { PathTreeItem(id: $0.id, path: $0.path) },
            selection: $selection
        )
        .accessibilityIdentifier("review-file-tree")
    }
}
