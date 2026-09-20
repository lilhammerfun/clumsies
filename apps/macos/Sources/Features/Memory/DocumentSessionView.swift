import AppKit
import SwiftUI

struct DocumentSessionView: View {
    @EnvironmentObject private var memoryCatalog: MemoryCatalog
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var draftStore: DraftStore
    @EnvironmentObject private var workspaceFeedback: WorkspaceFeedback
    @EnvironmentObject private var memoryModel: MemoryModel
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation
    @EnvironmentObject private var reviewModel: ReviewsModel
    @EnvironmentObject private var documentSessions: DocumentSessions
    let item: MemoryListItem
    let mode: WorkbenchTabMode

    @StateObject private var model: DocumentEditorModel
    @State private var reviewDraft: LocalDraft?
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
        documentContent
            .frame(maxWidth: .infinity, maxHeight: .infinity)
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
            Text(MemoryFileTreeAlert.organizationDeletion(items: [item]).message)
        }
    }

    private var documentContent: some View {
        Group {
            if self.mode == .diff {
                self.documentDiff
            } else if self.item.draft?.isDeletion == true {
                ContentUnavailableView(
                    "Marked for deletion",
                    systemImage: "trash",
                    description: Text(
                        self.item.draft?.scope == .org
                            ? "This file will be deleted from remote Memory when the Review is merged.\nChoose Discard Draft to cancel the deletion."
                            : "Choose Discard Draft to cancel the deletion."
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
        case .draft: String(localized: "Draft")
        case .shared: String(localized: "Remote")
        case .draftAndShared: String(localized: "Draft + Remote")
        }
        switch (change.from, change.to) {
        case let (from?, to?):
            return String(localized: "\(owner) path: \(from) → \(to)")
        case let (nil, to?):
            return String(localized: "\(owner) added: \(to)")
        case let (from?, nil):
            return String(localized: "\(owner) deleted: \(from)")
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

}
