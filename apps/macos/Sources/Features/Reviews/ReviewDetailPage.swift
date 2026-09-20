import SwiftUI

struct ReviewDetailPage: View {
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var workspaceFeedback: WorkspaceFeedback
    @EnvironmentObject private var reviewModel: ReviewsModel
    let reviewId: String
    let loadsRemoteContent: Bool

    @StateObject private var model: ReviewDetailModel

    init(reviewId: String, loadsRemoteContent: Bool = true, model: @autoclosure @escaping () -> ReviewDetailModel) {
        self.reviewId = reviewId
        self.loadsRemoteContent = loadsRemoteContent
        _model = StateObject(wrappedValue: model())
    }

    var body: some View {
        Group {
            if self.model.loading {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let loadError = model.loadError {
                ContentUnavailableView {
                    Label("Unable to Load Review", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(loadError)
                } actions: {
                    Button("Try Again") {
                        Task { await self.model.load() }
                    }
                }
            } else if let review = model.review, model.detail != nil, !model.draftDetails.isEmpty {
                self.content(review)
            } else {
                ContentUnavailableView(
                    "Review Unavailable",
                    systemImage: "checkmark.bubble",
                    description: Text("This Review is no longer in the workspace.")
                )
            }
        }
        .onChange(of: reviewModel.updates[reviewId] == nil) { _, finished in
            if finished, loadsRemoteContent { Task { await model.refreshDetail() } }
        }
        .task(id: reviewId) {
            guard self.loadsRemoteContent else {
                self.model.loading = false
                return
            }
            await self.model.load()
        }
        .task(id: reviewModel.updates[reviewId].map(ObjectIdentifier.init)) {
            guard loadsRemoteContent, let update = reviewModel.updates[reviewId] else { return }
            await reviewModel.prepareUpdate(update.review)
        }
        .onDisappear {
            self.model.invalidateDetailRequests()
        }
        .navigationTitle(model.review?.title ?? String(localized: "Review"))
        .onChange(of: reviewModel.pendingReviewReconciliationId) { _, reviewId in
            self.model.handlePendingReconciliation(reviewId)
        }
        .onChange(of: model.selectedFileId) { _, _ in
            self.model.selectCurrentFile()
        }
        .onChange(of: model.storedReviewDecisionSignature) { _, signature in
            guard let signature,
                  model.detail.map({ ReviewDecisionReadiness(review: WorkspaceLoader.mapReview($0.review)) })
                    != signature else { return }
            Task { await self.model.refreshDetail() }
        }
    }

    private func content(_ review: ReviewRecord) -> some View {
        return VStack(spacing: 0) {
            reviewHeader(review).padding(20)
            if review.freshness == .behind, review.reconciliation == .conflicts, !workspaceContext.isReviewAuthor(review) {
                Text("The author needs to resolve the conflicts in this Review.")
                    .font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 20).padding(.bottom, 12)
            }
            Divider()
            HSplitView {
            ReviewFileNavigator(
                files: self.model.fileDescriptors,
                selection: self.$model.selectedFileId
            )
            .frame(minWidth: 180, idealWidth: 220, maxWidth: 280)

            Group {
                if let selectedDraftDetail = model.selectedDraftDetail {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 20) {
                            if self.model.showsGeneralComments {
                                self.generalCommentsPanel
                            }

                            if let update = reviewModel.updates[reviewId] {
                                ReviewUpdateView(model: update, draftId: selectedDraftDetail.draft.draftId) {
                                    self.diffPanel(detail: selectedDraftDetail)
                                }
                            } else {
                                self.diffPanel(detail: selectedDraftDetail)
                            }
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
        }
        .onAppear {
            if self.model.selectedFileId == nil {
                self.model.selectedFileId = self.model.fileDescriptors.first?.id
            }
        }
    }

    private func reviewHeader(_ review: ReviewRecord) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(review.title)
                    .font(.title2.weight(.semibold))
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

                Button(action: self.model.toggleGeneralComments) {
                    Image(systemName: self.model.reviewWideCommentCount == 0 ? "bubble.badge.plus" : "bubble")
                }
                .buttonStyle(.borderless)
                .help(self.model.reviewWideCommentCount == 0
                    ? "Add a review-wide comment"
                    : "Show \(self.model.reviewWideCommentCount) review-wide comments")
                .accessibilityLabel(self.model.reviewWideCommentCount == 0
                    ? "Add a review-wide comment"
                    : "Show \(self.model.reviewWideCommentCount) review-wide comments")
            }

            self.metadata(review)

            let description = review.description.trimmingCharacters(in: .whitespacesAndNewlines)
            if !description.isEmpty {
                Text(description)
                    .foregroundStyle(.primary)
                    .textSelection(.enabled)
            }

            if review.status != "open" {
                self.decisionSummary(review)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .layoutPriority(1)
    }

    private func metadata(_ review: ReviewRecord) -> some View {
        let author = review.author.displayName ?? review.author.email
        let project = workspaceContext.projects.first { $0.id == review.projectId }?.name
        let context = [author, project]
            .compactMap { $0 }
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
        let updated = TimestampFormatting.absoluteText(review.updatedAt)
            .map { String(localized: " · Updated \($0)") } ?? ""
        return Text("\(context)\(updated)")
        .font(.caption)
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .truncationMode(.tail)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func decisionSummary(_ review: ReviewRecord) -> some View {
        let decisionTime = review.decidedAt.flatMap {
            TimestampFormatting.absoluteText($0)
        }
        return VStack(alignment: .leading, spacing: 7) {
            if review.status != "merged" {
                HStack(spacing: 7) {
                    Image(systemName: self.decisionSymbol(review.status))
                        .foregroundStyle(self.decisionColor(review.status))
                    Text(self.decisionTitle(review.status))
                        .font(.callout.weight(.semibold))
                    if let decider = review.decidedBy {
                        let deciderName = decider.displayName ?? decider.email
                        Text("· Decision by \(deciderName)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    if let decidedAt = review.decidedAt,
                       let decisionTime {
                        Text("· \(decisionTime)")
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

                if self.model.composing != .general {
                    Button {
                        self.model.composing = .general
                        self.model.commentDraft = ""
                    } label: {
                        Image(systemName: "plus")
                    }
                    .buttonStyle(.borderless)
                    .help("Add a review-wide comment")
                    .accessibilityLabel("Add a review-wide comment")
                }
            }

            if self.model.composing == .general {
                ReviewCommentComposer(
                    text: self.$model.commentDraft,
                    isSubmitting: self.model.isSubmittingComment,
                    onCancel: { self.model.composing = nil; self.model.commentDraft = "" },
                    onSubmit: { Task { await self.model.submitComment(line: nil) } }
                )
            }

            ForEach(self.model.generalComments) { comment in
                ReviewCommentRow(comment: comment) {
                    self.model.composing = .general
                    self.model.commentDraft = ""
                }
            }

            if !self.model.unplacedComments.isEmpty {
                Text("Comments from an earlier revision or file path")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                ForEach(self.model.unplacedComments) { comment in
                    VStack(alignment: .leading, spacing: 4) {
                        if let path = comment.anchorPath, let line = comment.anchorLine {
                            Text("\(path):\(line)")
                                .font(.caption.monospaced())
                                .foregroundStyle(.secondary)
                                .textSelection(.enabled)
                        }
                        ReviewCommentRow(comment: comment) {
                            self.model.composing = .general
                            self.model.commentDraft = ""
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func diffPanel(detail: ReviewDraftDetail) -> some View {
        if model.loadingFile {
            ProgressView("Loading file changes…")
                .frame(maxWidth: .infinity)
                .padding(.vertical, 20)
        } else if let fileLoadError = model.fileLoadError {
            VStack(alignment: .leading, spacing: 8) {
                Label("Unable to Load File", systemImage: "exclamationmark.triangle")
                Text(fileLoadError).foregroundStyle(.secondary).textSelection(.enabled)
                Button("Try Again") { self.model.selectCurrentFile() }
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
        } else if let diffModel = model.diffModel {
            UnifiedDiffView(
                model: diffModel,
                commentsByLine: model.commentsByLine,
                composingLine: model.composingLine,
                commentDraft: $model.commentDraft,
                isSubmittingComment: model.isSubmittingComment,
                onRequestComment: { self.model.composing = .line($0) },
                onCancelComment: { self.model.composing = nil; self.model.commentDraft = "" },
                onSubmitComment: { line in Task { await self.model.submitComment(line: line) } },
                onReply: { line in self.model.composing = .line(line) }
            )
        } else if model.changeSources != nil {
            Text(detail.operations.isEmpty
                 ? "Remote already includes this file’s changes."
                 : "This Review changes metadata without changing text content.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .padding(.vertical, 20)
        }
    }

    private func decisionTitle(_ status: String) -> String {
        switch status {
        case "approved": String(localized: "Approved")
        case "rejected": String(localized: "Changes requested")
        case "merged": String(localized: "Merged")
        default: ReviewStatusIndicator.title(for: status)
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
            items: files.map { file in
                PathTreeItem(id: file.id, path: file.path,
                             badge: file.reconciliationState?.title,
                             badgeColor: file.reconciliationState?.badgeColor)
            },
            selection: $selection
        )
        .accessibilityIdentifier("review-file-tree")
    }
}
