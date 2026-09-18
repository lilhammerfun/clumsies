import AppKit
import SwiftUI

struct DocumentSessionView: View {
    @EnvironmentObject private var memoryCatalog: MemoryCatalog
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var draftStore: DraftStore
    @EnvironmentObject private var workspaceFeedback: WorkspaceFeedback
    @EnvironmentObject private var memoryModel: MemoryModel
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation
    @EnvironmentObject private var reconciler: DraftReconciliationService
    @EnvironmentObject private var reviewModel: ReviewsModel
    @EnvironmentObject private var documentSessions: DocumentSessions
    let item: MemoryListItem
    let mode: WorkbenchTabMode

    @StateObject private var model: DocumentEditorModel
    @State private var reviewDraft: LocalDraft?
    @State private var reconciliationUpdateRequest = 0
    @State private var documentDiffRetryRequest = 0
    @State private var confirmsOrganizationDeletion = false

    private var sessionKey: MemoryDocumentSessionKey? {
        documentSessions.documentSessionKey(for: item)
    }

    init(item: MemoryListItem, mode: WorkbenchTabMode, model: @autoclosure @escaping () -> DocumentEditorModel) {
        self.item = item
        self.mode = mode
        _model = StateObject(wrappedValue: model())
    }

    var body: some View {
        Group {
            if let candidate = documentSessions.pendingDocumentReconciliationCandidates[item.id] {
                DraftReconciliationView(
                    candidate: candidate,
                    updateRequest: self.reconciliationUpdateRequest,
                    usesContextualUpdateAction: true,
                    initialResolvedState: self.documentSessions.documentReconciliationResolution(for: self.item.id),
                    onResolvedStateChange: {
                        self.documentSessions.updateDocumentReconciliationResolution($0, for: self.item.id)
                    },
                    onUpdateStateChange: self.publishReconciliationToolbarState,
                    onCancel: self.closeReconciliation,
                    onApplied: self.closeReconciliation
                ) { resolvedState in
                    try await self.reconciler.applyReconciliation(
                        draftId: candidate.draftId,
                        candidate: candidate,
                        resolvedState: resolvedState,
                        documentItemId: self.item.id
                    )
                }
                .id(candidate.candidateId)
            } else {
                self.documentContent
            }
        }
        .onChange(of: item.document) { _, latest in
            self.model.adoptAuthoritativeDocument(latest)
        }
        .onChange(of: memoryCatalog.documentContentGeneration(for: item.id)) { _, _ in
            self.model.adoptAuthoritativeDocument(self.item.document)
        }
        .onChange(of: workspaceNavigation.pendingDocumentCommand) { _, command in
            self.handleDocumentCommand(command)
        }
        .onAppear {
            self.handleDocumentCommand(self.workspaceNavigation.pendingDocumentCommand)
        }
        .onDisappear {
            self.model.flushSave(item: self.item, mode: self.mode)
            self.clearReconciliationToolbarState()
        }
        .sheet(item: $reviewDraft) { draft in
            ReviewRequestSheet(
                initialTitle: self.model.document.title,
                loadCandidates: { [try await self.model.loadReviewCandidate(draft, item: self.item)] }
            ) { title, description, reconciliations in
                let reconciliation = reconciliations.first
                try await self.model.submitReview(
                    draft,
                    item: self.item,
                    title: title,
                    description: description,
                    candidate: reconciliation?.candidate,
                    resolvedState: reconciliation?.resolvedState
                )
            }
        }
        .alert("Delete File?", isPresented: $confirmsOrganizationDeletion) {
            Button("Cancel", role: .cancel) {}
            Button("Delete", role: .destructive) {
                self.model.moveToTrash(item: self.item)
            }
        } message: {
            Text(
                "This creates a deletion draft proposal. If reviewed and merged, "
                    + "the organization memory will be removed for every project that includes it."
            )
        }
    }

    private var documentContent: some View {
        Group {
            if self.mode == .diff {
                self.documentDiff
            } else if self.item.draft?.isDeletion == true {
                ContentUnavailableView(
                    self.item.draft?.scope == .org
                        ? "Pending organization deletion"
                        : "Pending deletion",
                    systemImage: "trash",
                    description: Text(
                        self.item.draft?.scope == .org
                            ? "Discard the draft proposal to keep this organization memory."
                            : "Discard the draft to keep this memory."
                    )
                )
            } else if self.mode == .preview {
                MarkdownPreview(source: self.renderedSource)
            } else {
                self.editor
                    .disabled(
                        !self.draftStore.canEditMemory(self.item)
                            || self.workspaceContext.isSwitchingMemoryContext
                            || self.documentSessions.isSynchronizingDocument(self.item.id)
                    )
            }
        }
    }

    @ViewBuilder
    private var documentDiff: some View {
        let identity = documentDiffIdentity
        // The pane contract for a document view is the same one Preview
        // uses: the root is a greedy vertical ScrollView that fills the
        // remaining height, with content pinned to the top. The unified
        // diff stays a content-sized fragment (as in Reviews) and this
        // outer scroll handles vertical overflow.
        GeometryReader { geometry in
            ScrollView([.vertical]) {
                VStack(alignment: .leading, spacing: 0) {
                    if !self.model.documentPathChanges.isEmpty {
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(self.model.documentPathChanges.indices, id: \.self) { index in
                                HStack(spacing: 6) {
                                    Image(systemName: "arrow.right")
                                        .foregroundStyle(.secondary)
                                    Text(self.pathChangeSummary(self.model.documentPathChanges[index]))
                                        .textSelection(.enabled)
                                }
                            }
                        }
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 12)
                        .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
                        .background(Color.accentColor.opacity(0.06))
                    }

                    if let presentation = model.documentDiffPresentation,
                       presentation.changedLineCount > 0 {
                        UnifiedDiffView(presentation: presentation)
                    } else if self.model.loadsDocumentDiff {
                        ProgressView()
                            .controlSize(.small)
                    } else if let documentDiffError = model.documentDiffError {
                        ContentUnavailableView {
                            Label("Unable to Load Diff", systemImage: "exclamationmark.triangle")
                        } description: {
                            Text(documentDiffError)
                        } actions: {
                            Button("Retry") { self.documentDiffRetryRequest += 1 }
                        }
                    } else if self.model.documentPathChanges.isEmpty {
                        ContentUnavailableView(
                            "No Changes",
                            systemImage: "doc.text",
                            description: Text("No local or remote changes to show for this document.")
                        )
                    }
                }
                .frame(
                    maxWidth: .infinity,
                    minHeight: geometry.size.height,
                    alignment: self.centersDocumentDiffStatus ? .center : .topLeading
                )
            }
        }
        .task(id: identity) {
            await self.model.loadDocumentDiff(for: identity)
        }
    }

    private var centersDocumentDiffStatus: Bool {
        model.documentPathChanges.isEmpty
            && (model.documentDiffPresentation?.changedLineCount ?? 0) == 0
    }

    /// A path-only change has no content diff. Surface draft and shared
    /// additions, removals, and renames so the pane never looks empty.
    private func pathChangeSummary(_ change: DocumentPathChange) -> String {
        let owner = switch change.source {
        case .draft: "Draft"
        case .shared: "Shared"
        case .draftAndShared: "Draft + Shared"
        }
        switch (change.from, change.to) {
        case let (from?, to?):
            return "\(owner) path: \(from) → \(to)"
        case let (nil, to?):
            return "\(owner) added: \(to)"
        case let (from?, nil):
            return "\(owner) deleted: \(from)"
        case (nil, nil):
            return owner
        }
    }

    private var documentDiffIdentity: DocumentDiffIdentity {
        DocumentDiffIdentity(
            item: item,
            localDocument: model.document,
            staleResourceGeneration: item.resource.flatMap {
                self.memoryCatalog.staleResourceGeneration(for: $0.id)
            },
            retryRequest: documentDiffRetryRequest
        )
    }

    @ViewBuilder
    private var editor: some View {
        switch item.kind {
        case .context, .rules, .workflows:
            NativeTextEditor(text: editorText)
        }
    }

    private var editorText: Binding<String> {
        Binding(
            get: { self.model.document.body },
            set: { nextBody in
                guard nextBody != self.model.document.body else { return }
                var nextDocument = self.model.document
                nextDocument.body = nextBody
                self.model.document = nextDocument
                self.model.stageSave(nextDocument, item: self.item, mode: self.mode)
            }
        )
    }

    private var renderedSource: String {
        switch item.kind {
        case .context, .rules, .workflows:
            return model.document.body
        }
    }

    private func handleDocumentCommand(_ command: DocumentSessionCommand?) {
        guard let command, command.sessionKey == sessionKey else { return }
        workspaceNavigation.pendingDocumentCommand = nil
        switch command {
        case .requestReview(_, let draft):
            reviewDraft = draft
        case .discardDraft(_, let draft):
            model.discard(draft, item: item)
        case .applyReconciliation:
            reconciliationUpdateRequest += 1
        case .closeReconciliation:
            closeReconciliation()
        case .moveToTrash:
            guard draftStore.canEditMemory(item),
                  MemoryFileTreeMenu.canProposeOrganizationDeletion(
                      item,
                      inOrgView: workspaceContext.activeProjectId == nil
                  ) else {
                return
            }
            confirmsOrganizationDeletion = true
        }
    }

    private func publishReconciliationToolbarState(canUpdate: Bool, isUpdating: Bool) {
        guard let sessionKey else { return }
        workspaceNavigation.documentReconciliationToolbarState = .init(
            sessionKey: sessionKey,
            isLoading: false,
            canUpdate: canUpdate,
            isUpdating: isUpdating
        )
    }

    private func closeReconciliation() {
        guard let sessionKey else { return }
        documentSessions.finishDocumentReconciliation(for: sessionKey)
        clearReconciliationToolbarState()
    }

    private func clearReconciliationToolbarState() {
        guard workspaceNavigation.documentReconciliationToolbarState?.sessionKey == sessionKey else { return }
        workspaceNavigation.documentReconciliationToolbarState = nil
    }

}
