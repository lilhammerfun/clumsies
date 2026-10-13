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
    private var baseItem: MemoryListItem
    private var baseCommitId: String?

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
        let pending = sessions.documentSessionKey(for: item).flatMap { drafts.pendingDocumentSaves[$0] }
        document = pending?.document ?? item.document
        authoritativeDocument = item.document
        baseItem = pending?.item ?? item
        if let pending { baseCommitId = pending.baseCommitId }
        else { captureBase(for: item) }
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
        } catch where error.isUserCancellation {
            // `.task(id:)` immediately starts a replacement for a changed
            // identity. Let that task remain the owner of loading state.
        } catch {
            guard !Task.isCancelled,
                  diffGeneration == generation else { return }
            documentDiffError = error.actionMessage
            loadsDocumentDiff = false
        }
    }

    func adoptAuthoritativeDocument(_ latest: EditableMemoryDocument, item: MemoryListItem? = nil) {
        let previous = authoritativeDocument
        authoritativeDocument = latest
        // A resource sync can replace the authoritative document while this
        // session stays alive. Adopt it only when the editor still matches the
        // previous snapshot so an in-flight local edit is never lost.
        if document == previous {
            document = latest
            if let item { captureBase(for: item) }
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

    private func captureBase(for item: MemoryListItem) {
        baseItem = item
        if let resource = item.resource, resource.scope == .project {
            baseItem.editBasePin = resource.baseCommitPin
        } else {
            baseItem.editBasePin = item.editBasePin ?? workspaceContext.activeProject?.baseCommitPin
        }
        if let draft = item.draft { baseCommitId = draft.baseCommitId }
        else if let resource = item.resource, resource.scope == .project {
            baseCommitId = resource.refCommitId
        } else {
            baseCommitId = baseItem.editBasePin?.commitId ?? workspaceContext.activeProject?.refCommitId
        }
    }

    func stageSave(_ nextDocument: EditableMemoryDocument, item: MemoryListItem, mode: WorkbenchTabMode) {
        guard !suppressesSaving,
              draftStore.canEditMemory(item),
              !workspaceContext.isSwitchingMemoryContext,
              !documentSessions.isSynchronizingDocument(item.id),
              mode == .source,
              nextDocument != item.document else { return }
        draftStore.stageDocumentSave(baseItem, document: nextDocument, capturedBaseCommitId: .some(baseCommitId))
    }

    func flushSave(item: MemoryListItem, mode: WorkbenchTabMode) {
        guard !suppressesSaving,
              mode == .source else { return }
        Task {
            do {
                try await self.draftStore.flushDocumentSave(item)
            } catch {
                self.workspaceFeedback.errorMessage = error.actionMessage
            }
        }
    }

    func submitReview(
        _ draft: LocalDraft,
        item: MemoryListItem,
        title: String,
        description: String,
        candidate: DraftReconciliationCandidate?,
        resolvedState: ReconciliationResourceState?,
        contributions: [OrgContributionEntry] = []
    ) async throws {
        try await draftStore.flushDocumentSave(item)
        let latest = draftStore.drafts.first { $0.id == draft.id } ?? draft
        try await reviewModel.requestReview(
            for: latest,
            title: title,
            description: description,
            candidate: candidate,
            resolvedState: resolvedState,
            contributions: contributions
        )
    }

    func loadReviewCandidates(_ draft: LocalDraft, item: MemoryListItem) async throws -> [DraftReconciliationCandidate] {
        try await draftStore.flushDocumentSave(item)
        let latest = draftStore.drafts.first { $0.id == draft.id } ?? draft
        return try await reconciler.reconciliationCandidates(for: [latest])
    }

    func discard(_ draft: LocalDraft, item: MemoryListItem) {
        suppressesSaving = true
        Task {
            await self.draftStore.discard(draft)
            self.suppressesSaving = false
        }
    }

    func moveToTrash(item: MemoryListItem) {
        guard let activeProjectId = workspaceContext.activeProjectId,
              item.projectContextId == activeProjectId,
              draftStore.canEditMemory(item),
              MemoryFileTreeMenu.canProposeMemoryDeletion(
                  item,
                  inOrgView: false
              ) else {
            return
        }
        suppressesSaving = true
        Task {
            await self.draftStore.delete(item)
            self.suppressesSaving = false
        }
    }

}
