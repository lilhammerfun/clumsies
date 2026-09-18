import AppKit
import SwiftUI

private struct DocumentDiffIdentity: Hashable {
    let item: MemoryListItem
    let localDocument: EditableMemoryDocument
    let staleResourceGeneration: UUID?
    let retryRequest: Int
}

struct DocumentSessionView: View {
    let store: WorkspaceCoordinator
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

    @State private var document: EditableMemoryDocument
    @State private var authoritativeDocument: EditableMemoryDocument
    @State private var suppressesSaving = false
    @State private var reviewDraft: LocalDraft?
    @State private var reconciliationUpdateRequest = 0
    @State private var documentDiffPresentation: UnifiedDiffPresentation?
    @State private var documentPathChanges: [DocumentPathChange] = []
    @State private var loadsDocumentDiff = false
    @State private var documentDiffError: String?
    @State private var documentDiffRetryRequest = 0
    @State private var confirmsOrganizationDeletion = false

    private var sessionKey: MemoryDocumentSessionKey? {
        documentSessions.documentSessionKey(for: item)
    }

    init(store: WorkspaceCoordinator, item: MemoryListItem, mode: WorkbenchTabMode) {
        self.store = store
        self.item = item
        self.mode = mode
        _document = State(initialValue: store.edits.pendingDocument(for: item) ?? item.document)
        _authoritativeDocument = State(initialValue: item.document)
    }

    var body: some View {
        Group {
            if let candidate = documentSessions.pendingDocumentReconciliationCandidates[item.id] {
                DraftReconciliationView(
                    candidate: candidate,
                    updateRequest: reconciliationUpdateRequest,
                    usesContextualUpdateAction: true,
                    initialResolvedState: documentSessions.documentReconciliationResolution(for: item.id),
                    onResolvedStateChange: {
                        documentSessions.updateDocumentReconciliationResolution($0, for: item.id)
                    },
                    onUpdateStateChange: publishReconciliationToolbarState,
                    onCancel: closeReconciliation,
                    onApplied: closeReconciliation
                ) { resolvedState in
                    try await reconciler.applyReconciliation(
                        draftId: candidate.draftId,
                        candidate: candidate,
                        resolvedState: resolvedState,
                        documentItemId: item.id
                    )
                }
                .id(candidate.candidateId)
            } else {
                documentContent
            }
        }
        .onChange(of: item.document) { _, latest in
            adoptAuthoritativeDocument(latest)
        }
        .onChange(of: memoryCatalog.documentContentGeneration(for: item.id)) { _, _ in
            adoptAuthoritativeDocument(item.document)
        }
        .onChange(of: workspaceNavigation.pendingDocumentCommand) { _, command in
            handleDocumentCommand(command)
        }
        .onAppear {
            handleDocumentCommand(workspaceNavigation.pendingDocumentCommand)
        }
        .onDisappear {
            flushSave()
            clearReconciliationToolbarState()
        }
        .sheet(item: $reviewDraft) { draft in
            ReviewRequestSheet(
                initialTitle: document.title,
                loadCandidates: { [try await loadReviewCandidate(draft)] }
            ) { title, description, reconciliations in
                let reconciliation = reconciliations.first
                try await submitReview(
                    draft,
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
                moveToTrash()
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
            if mode == .diff {
                documentDiff
            } else if item.draft?.isDeletion == true {
                ContentUnavailableView(
                    item.draft?.scope == .org
                        ? "Pending organization deletion"
                        : "Pending deletion",
                    systemImage: "trash",
                    description: Text(
                        item.draft?.scope == .org
                            ? "Discard the draft proposal to keep this organization memory."
                            : "Discard the draft to keep this memory."
                    )
                )
            } else if mode == .preview {
                MarkdownPreview(source: renderedSource)
            } else {
                editor
                    .disabled(
                        !draftStore.canEditMemory(item)
                            || workspaceContext.isSwitchingMemoryContext
                            || documentSessions.isSynchronizingDocument(item.id)
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
                    if !documentPathChanges.isEmpty {
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(documentPathChanges.indices, id: \.self) { index in
                                HStack(spacing: 6) {
                                    Image(systemName: "arrow.right")
                                        .foregroundStyle(.secondary)
                                    Text(pathChangeSummary(documentPathChanges[index]))
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

                    if let presentation = documentDiffPresentation,
                       presentation.changedLineCount > 0 {
                        UnifiedDiffView(presentation: presentation)
                    } else if loadsDocumentDiff {
                        ProgressView()
                            .controlSize(.small)
                    } else if let documentDiffError {
                        ContentUnavailableView {
                            Label("Unable to Load Diff", systemImage: "exclamationmark.triangle")
                        } description: {
                            Text(documentDiffError)
                        } actions: {
                            Button("Retry") { documentDiffRetryRequest += 1 }
                        }
                    } else if documentPathChanges.isEmpty {
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
                    alignment: centersDocumentDiffStatus ? .center : .topLeading
                )
            }
        }
        .task(id: identity) {
            await loadDocumentDiff(for: identity)
        }
    }

    private var centersDocumentDiffStatus: Bool {
        documentPathChanges.isEmpty
            && (documentDiffPresentation?.changedLineCount ?? 0) == 0
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
            localDocument: document,
            staleResourceGeneration: item.resource.flatMap {
                memoryCatalog.staleResourceGeneration(for: $0.id)
            },
            retryRequest: documentDiffRetryRequest
        )
    }

    private func loadDocumentDiff(for identity: DocumentDiffIdentity) async {
        guard mode == .diff else { return }
        documentDiffPresentation = nil
        documentDiffError = nil
        documentPathChanges = memoryModel.documentPathChanges(for: identity.item)
        loadsDocumentDiff = true

        do {
            let result = try await memoryModel.documentDiffPresentation(
                for: identity.item,
                localText: identity.localDocument.body
            )
            try Task.checkCancellation()
            guard identity == documentDiffIdentity, mode == .diff else { return }
            documentDiffPresentation = result?.presentation
            documentPathChanges = result?.pathChanges ?? []
            loadsDocumentDiff = false
        } catch is CancellationError {
            // `.task(id:)` immediately starts a replacement for a changed
            // identity. Let that task remain the owner of loading state.
        } catch {
            guard !Task.isCancelled,
                  identity == documentDiffIdentity,
                  mode == .diff else { return }
            documentDiffError = error.localizedDescription
            loadsDocumentDiff = false
        }
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
            get: { document.body },
            set: { nextBody in
                guard nextBody != document.body else { return }
                var nextDocument = document
                nextDocument.body = nextBody
                document = nextDocument
                stageSave(nextDocument)
            }
        )
    }

    private var renderedSource: String {
        switch item.kind {
        case .context, .rules, .workflows:
            return document.body
        }
    }

    private func adoptAuthoritativeDocument(_ latest: EditableMemoryDocument) {
        let previous = authoritativeDocument
        authoritativeDocument = latest
        // A resource sync can replace the authoritative document while this
        // session stays alive. Adopt it only when the editor still matches the
        // previous snapshot so an in-flight local edit is never lost.
        if document == previous {
            document = latest
            return
        }
        // A file-tree rename is independent of a dirty Source body. Merge
        // path/title changes from the authoritative draft while keeping the
        // user's in-flight text, so the next autosave cannot rename it back.
        if document.path == previous.path {
            document.path = latest.path
        }
        if document.title == previous.title {
            document.title = latest.title
        }
    }

    private func handleDocumentCommand(_ command: DocumentSessionCommand?) {
        guard let command, command.sessionKey == sessionKey else { return }
        workspaceNavigation.pendingDocumentCommand = nil
        switch command {
        case .requestReview(_, let draft):
            reviewDraft = draft
        case .discardDraft(_, let draft):
            discard(draft)
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

    private func stageSave(_ nextDocument: EditableMemoryDocument) {
        guard !suppressesSaving,
              draftStore.canEditMemory(item),
              !workspaceContext.isSwitchingMemoryContext,
              !documentSessions.isSynchronizingDocument(item.id),
              mode == .source,
              nextDocument != item.document else { return }
        draftStore.stageDocumentSave(item, document: nextDocument)
    }

    private func flushSave() {
        guard !suppressesSaving,
              mode == .source else { return }
        Task {
            do {
                try await draftStore.flushDocumentSave(item)
            } catch {
                workspaceFeedback.errorMessage = error.localizedDescription
            }
        }
    }

    private func submitReview(
        _ draft: LocalDraft,
        title: String,
        description: String,
        candidate: DraftReconciliationCandidate?,
        resolvedState: ReconciliationResourceState?
    ) async throws {
        try await draftStore.flushDocumentSave(item)
        let latest = draftStore.drafts.first { $0.id == draft.id } ?? draft
        try await reviewModel.requestReview(
            for: latest,
            title: title,
            description: description,
            candidate: candidate,
            resolvedState: resolvedState
        )
    }

    private func loadReviewCandidate(_ draft: LocalDraft) async throws -> DraftReconciliationCandidate {
        try await draftStore.flushDocumentSave(item)
        let latest = draftStore.drafts.first { $0.id == draft.id } ?? draft
        return try await reconciler.reconciliationCandidate(for: latest)
    }

    private func discard(_ draft: LocalDraft) {
        suppressesSaving = true
        draftStore.cancelDocumentSave(item)
        Task {
            await draftStore.discard(draft)
            suppressesSaving = false
        }
    }

    private func moveToTrash() {
        guard let activeProjectId = workspaceContext.activeProjectId,
              item.projectContextId == activeProjectId,
              draftStore.canEditMemory(item),
              MemoryFileTreeMenu.canProposeOrganizationDeletion(
                  item,
                  inOrgView: false
              ) else {
            return
        }
        suppressesSaving = true
        draftStore.cancelDocumentSave(item)
        Task {
            await draftStore.delete(item)
            suppressesSaving = false
        }
    }
}
