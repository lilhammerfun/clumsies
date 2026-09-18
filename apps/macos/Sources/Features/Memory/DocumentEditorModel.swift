import Combine
import Foundation

struct DocumentDiffIdentity: Hashable {
    let item: MemoryListItem
    let localDocument: EditableMemoryDocument
    let staleResourceGeneration: UUID?
    let retryRequest: Int
}

@MainActor
final class DocumentEditorModel: ObservableObject {
    private let draftStore: DraftStore
    private let workspaceContext: WorkspaceContext
    private let workspaceFeedback: WorkspaceFeedback
    private let documentSessions: DocumentSessions
    private let memoryModel: MemoryModel
    private let reviewModel: ReviewsModel
    private let reconciler: DraftReconciliationService
    private var diffGeneration = UUID()

    init(item: MemoryListItem, drafts: DraftStore, context: WorkspaceContext,
         feedback: WorkspaceFeedback, sessions: DocumentSessions, memory: MemoryModel,
         reviews: ReviewsModel, reconciliation: DraftReconciliationService) {
        draftStore = drafts
        workspaceContext = context
        workspaceFeedback = feedback
        documentSessions = sessions
        memoryModel = memory
        reviewModel = reviews
        reconciler = reconciliation
        document = drafts.pendingDocument(for: item) ?? item.document
        authoritativeDocument = item.document
    }

    @Published var document: EditableMemoryDocument
    @Published private(set) var authoritativeDocument: EditableMemoryDocument
    @Published private(set) var suppressesSaving = false
    @Published private(set) var documentDiffPresentation: UnifiedDiffPresentation?
    @Published private(set) var documentPathChanges: [DocumentPathChange] = []
    @Published private(set) var loadsDocumentDiff = false
    @Published private(set) var documentDiffError: String?

    func loadDocumentDiff(for identity: DocumentDiffIdentity) async {
        let generation = UUID()
        diffGeneration = generation
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
            guard diffGeneration == generation else { return }
            documentDiffPresentation = result?.presentation
            documentPathChanges = result?.pathChanges ?? []
            loadsDocumentDiff = false
        } catch is CancellationError {
            // `.task(id:)` immediately starts a replacement for a changed
            // identity. Let that task remain the owner of loading state.
        } catch {
            guard !Task.isCancelled,
                  diffGeneration == generation else { return }
            documentDiffError = error.localizedDescription
            loadsDocumentDiff = false
        }
    }

    func adoptAuthoritativeDocument(_ latest: EditableMemoryDocument) {
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

    func stageSave(_ nextDocument: EditableMemoryDocument, item: MemoryListItem, mode: WorkbenchTabMode) {
        guard !suppressesSaving,
              draftStore.canEditMemory(item),
              !workspaceContext.isSwitchingMemoryContext,
              !documentSessions.isSynchronizingDocument(item.id),
              mode == .source,
              nextDocument != item.document else { return }
        draftStore.stageDocumentSave(item, document: nextDocument)
    }

    func flushSave(item: MemoryListItem, mode: WorkbenchTabMode) {
        guard !suppressesSaving,
              mode == .source else { return }
        Task {
            do {
                try await self.draftStore.flushDocumentSave(item)
            } catch {
                self.workspaceFeedback.errorMessage = error.localizedDescription
            }
        }
    }

    func submitReview(
        _ draft: LocalDraft,
        item: MemoryListItem,
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

    func loadReviewCandidate(_ draft: LocalDraft, item: MemoryListItem) async throws -> DraftReconciliationCandidate {
        try await draftStore.flushDocumentSave(item)
        let latest = draftStore.drafts.first { $0.id == draft.id } ?? draft
        return try await reconciler.reconciliationCandidate(for: latest)
    }

    func discard(_ draft: LocalDraft, item: MemoryListItem) {
        suppressesSaving = true
        draftStore.cancelDocumentSave(item)
        Task {
            await self.draftStore.discard(draft)
            self.suppressesSaving = false
        }
    }

    func moveToTrash(item: MemoryListItem) {
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
            await self.draftStore.delete(item)
            self.suppressesSaving = false
        }
    }

}
