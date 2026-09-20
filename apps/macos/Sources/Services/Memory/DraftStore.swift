import Combine
import Foundation

struct PendingDocumentSave {
    let item: MemoryListItem
    let document: EditableMemoryDocument
    let generation: UUID
}

@MainActor
final class DraftStore: ObservableObject {
    private let writeDraft: @Sendable (DaemonDraftOperationRequest) async throws -> DaemonDraftOperationResponse
    private let catalog: MemoryCatalog
    private let context: WorkspaceContext
    private let feedback: WorkspaceFeedback
    private let sessions: DocumentSessions
    let documentsChanged = PassthroughSubject<Void, Never>()
    let didSaveDocument = PassthroughSubject<String, Never>()
    let didDiscardDocument = PassthroughSubject<Void, Never>()

    init(
        catalog: MemoryCatalog, context: WorkspaceContext, feedback: WorkspaceFeedback, sessions: DocumentSessions,
        storeDraft: (@Sendable (DaemonDraftOperationRequest) async throws -> DaemonDraftOperationResponse)? = nil
    ) {
        self.catalog = catalog
        self.context = context
        self.feedback = feedback
        self.sessions = sessions
        let daemon = context.daemon
        writeDraft = storeDraft ?? { try await daemon.store($0) }
    }

    @Published var drafts: [LocalDraft] = []
    @Published var draftInventoryLoadState: WorkspaceCollectionLoadState = .loading

    var draftInventoryLoadTask: Task<Void, Never>?

    private let draftMutationGate = AsyncMutex()

    var pendingDocumentSaves: [MemoryDocumentSessionKey: PendingDocumentSave] = [:]

    private var documentSaveTasks: [MemoryDocumentSessionKey: Task<Void, Never>] = [:]

    func canCreateMemory(kind: MemoryKind, scope: MemoryScope) -> Bool {
        !context.isSwitchingMemoryContext && context.activeProjectId != nil && scope == .org
    }

    /// Authority is never edited in place: a Project member edits selected
    /// Organization Memory through a Project-bound LocalDraft. Legacy
    /// Project-scoped authority remains visible but read-only.
    func canEditMemory(_ item: MemoryListItem) -> Bool {
        guard context.phase == .ready else { return false }
        let projectContextId = item.projectContextId
            ?? (item.scope == .project ? item.projectId : nil)
        guard let projectContextId,
              projectContextId == context.activeProjectId else {
            return false
        }
        if sessions.projectOrgSelectionMutatingIds.contains(projectContextId) {
            return false
        }
        guard item.scope == .org else { return false }
        if let draft = item.draft, draft.targetId == nil { return true }
        return item.inherited
    }

    func pendingDocument(for item: MemoryListItem) -> EditableMemoryDocument? {
        guard let key = sessions.documentSessionKey(for: item) else { return nil }
        return pendingDocumentSaves[key]?.document
    }

    func stageDocumentSave(_ item: MemoryListItem, document: EditableMemoryDocument) {
        guard context.phase == .ready, !context.isSigningOut else { return }
        guard canEditMemory(item) else {
            feedback.errorMessage = String(localized: "You do not have permission to edit this memory.")
            return
        }
        guard sessions.synchronizationItemId(for: item) == nil else {
            feedback.errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
            return
        }
        guard !context.isSwitchingMemoryContext else {
            feedback.errorMessage = String(localized: "Wait for the Memory context switch to finish before editing.")
            return
        }
        guard let key = sessions.documentSessionKey(for: item) else {
            feedback.errorMessage = String(localized: "Open this memory from a Project before editing it.")
            return
        }
        let generation = UUID()
        pendingDocumentSaves[key] = .init(item: item, document: document, generation: generation)
        documentSaveTasks[key]?.cancel()
        documentSaveTasks[key] = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(600))
            guard !Task.isCancelled else { return }
            await self?.persistDocumentSave(key, generation: generation)
        }
    }

    func flushDocumentSave(_ item: MemoryListItem) async throws {
        guard let key = sessions.documentSessionKey(for: item) else { return }
        try await flushDocumentSave(key)
    }

    func flushDocumentSave(_ key: MemoryDocumentSessionKey) async throws {
        documentSaveTasks[key]?.cancel()
        documentSaveTasks[key] = nil
        guard let pending = pendingDocumentSaves[key] else { return }
        try await save(
            pending.item,
            document: pending.document,
            allowingDuringSynchronization: true,
            pendingSaveKey: key,
            pendingSaveGeneration: pending.generation
        )
    }

    func cancelDocumentSave(_ item: MemoryListItem) {
        guard let key = sessions.documentSessionKey(for: item) else { return }
        cancelDocumentSave(key)
    }

    func cancelDocumentSave(_ key: MemoryDocumentSessionKey) {
        documentSaveTasks[key]?.cancel()
        documentSaveTasks[key] = nil
        pendingDocumentSaves[key] = nil
    }

    private func finishPendingDocumentSaveIfCurrent(
        key: MemoryDocumentSessionKey?,
        generation: UUID?
    ) {
        guard let key, let generation,
              pendingDocumentSaves[key]?.generation == generation else { return }
        pendingDocumentSaves[key] = nil
        documentSaveTasks[key] = nil
    }

    func flushPendingDocumentChanges() async -> Bool {
        do {
            for key in Array(pendingDocumentSaves.keys) {
                try await flushDocumentSave(key)
            }
            return true
        } catch {
            feedback.errorMessage = error.localizedDescription
            return false
        }
    }

    func save(
        _ item: MemoryListItem,
        document: EditableMemoryDocument,
        allowingDuringSynchronization: Bool = false,
        pendingSaveKey: MemoryDocumentSessionKey? = nil,
        pendingSaveGeneration: UUID? = nil
    ) async throws {
        let flushesSelectionMutation = pendingSaveKey != nil
            && item.projectContextId.map(sessions.projectOrgSelectionMutatingIds.contains) == true
        guard canEditMemory(item) || flushesSelectionMutation else {
            throw ServerClientError.forbidden(String(localized: "You do not have permission to edit this memory."))
        }
        if context.isSwitchingMemoryContext, pendingSaveKey == nil {
            throw ServerClientError.forbidden(
                String(localized: "Wait for the Memory context switch to finish before editing.")
            )
        }
        if !allowingDuringSynchronization,
           sessions.synchronizationItemId(for: item) != nil {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        try validate(kind: item.kind, document: document)
        try await withDraftMutation {
            if let pendingSaveKey, let pendingSaveGeneration {
                guard self.pendingDocumentSaves[pendingSaveKey]?.generation
                    == pendingSaveGeneration else { return }
            }
            if !allowingDuringSynchronization,
               self.sessions.synchronizationItemId(for: item) != nil {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            let resource = item.resource
            let draft = self.currentDraft(for: item)
            guard let projectId = draftCarrierProjectId(for: item, currentDraft: draft) else {
                throw WorkspaceLoadError.noProjects
            }
            if item.draft != nil, draft == nil {
                self.finishPendingDocumentSaveIfCurrent(
                    key: pendingSaveKey,
                    generation: pendingSaveGeneration
                )
                return
            }
            if draft?.isDeletion == true {
                self.finishPendingDocumentSaveIfCurrent(
                    key: pendingSaveKey,
                    generation: pendingSaveGeneration
                )
                return
            }
            let draftId = draft?.id
            let projectRefCommitId = self.context.projects.first { $0.id == projectId }?.refCommitId
            let baseCommitId = draft?.baseCommitId
                ?? resource?.refCommitId
                ?? (item.scope == .org ? self.catalog.orgRefCommitId : projectRefCommitId)
            var response: DaemonDraftOperationResponse?

            if let resource, document.path != (draft?.document.path ?? resource.document.path) {
                response = try await self.storeDraft(
                    .init(
                        draftId: draftId,
                        baseCommitId: baseCommitId,
                        projectId: projectId,
                        scope: item.scope == .org ? .org : .project,
                        resource: item.kind.daemonKind,
                        op: .rename(id: resource.id, newPath: document.path, description: nil),
                        source: .desktop
                    )
                )
            }

            let targetId = resource?.id ?? draft?.targetId
            let operation: DaemonDraftOperation
            if let targetId {
                operation = .update(
                    id: targetId,
                    content: self.daemonContent(kind: item.kind, document: document),
                    description: nil
                )
            } else {
                operation = .create(
                    path: document.path,
                    content: self.daemonContent(kind: item.kind, document: document),
                    description: nil
                )
            }
            response = try await self.storeDraft(
                .init(
                    draftId: response?.draftId ?? draftId,
                    baseCommitId: baseCommitId,
                    projectId: projectId,
                    scope: item.scope == .org ? .org : .project,
                    resource: item.kind.daemonKind,
                    op: operation,
                    source: .desktop
                )
            )
            if let response {
                try await self.refreshDraft(response.draftId)
                if self.context.activeProjectId == projectId {
                    self.didSaveDocument.send(resource?.id ?? response.draftId)
                }
            }
            self.finishPendingDocumentSaveIfCurrent(
                key: pendingSaveKey,
                generation: pendingSaveGeneration
            )
        }
    }

    /// Renames without coupling the path mutation to whatever body happens to
    /// be loaded in the file tree. Target-backed resources use their stable
    /// resource id; a pure create Draft uses its provisional Draft id. Both
    /// keep content and semantic metadata untouched.
    func rename(_ item: MemoryListItem, to newPath: String) async throws {
        guard !context.isSwitchingMemoryContext else {
            throw ServerClientError.forbidden(
                String(localized: "Wait for the Memory context switch to finish before renaming.")
            )
        }
        guard canEditMemory(item) else {
            throw ServerClientError.forbidden(String(localized: "You do not have permission to rename this memory."))
        }
        guard let sessionKey = sessions.documentSessionKey(for: item) else {
            throw ServerClientError.forbidden(
                String(localized: "Open this memory from a Project before renaming it.")
            )
        }
        guard sessions.synchronizationItemId(for: item) == nil else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        try validatePath(kind: item.kind, path: newPath)

        // Preserve a dirty editor before changing the path. The draft gate in
        // save prevents an already-running debounce and this explicit flush
        // from persisting the same generation twice.
        if pendingDocumentSaves[sessionKey] != nil {
            try await flushDocumentSave(sessionKey)
        }

        try await withDraftMutation {
            guard self.sessions.synchronizationItemId(for: item) == nil else {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            let resource = item.resource
            let draft = self.currentDraft(for: item)
            if item.draft != nil, draft == nil { return }
            if draft?.isDeletion == true { return }
            guard let plan = Self.documentRenamePlan(
                for: item,
                currentDraft: draft,
                newPath: newPath
            ) else {
                throw MemoryValidationError.memoryCannotBeRenamed
            }
            guard let projectId = draftCarrierProjectId(for: item, currentDraft: draft) else {
                throw WorkspaceLoadError.noProjects
            }
            let currentPath = draft?.document.path ?? resource?.document.path
            guard currentPath != newPath else { return }

            let projectRefCommitId = self.context.projects.first { $0.id == projectId }?.refCommitId
            let response = try await self.storeDraft(
                .init(
                    draftId: draft?.id,
                    baseCommitId: draft?.baseCommitId
                        ?? resource?.refCommitId
                        ?? (item.scope == .org ? self.catalog.orgRefCommitId : projectRefCommitId),
                    projectId: projectId,
                    scope: item.scope == .org ? .org : .project,
                    resource: item.kind.daemonKind,
                    op: .rename(id: plan.targetId, newPath: plan.newPath, description: nil),
                    source: .desktop
                )
            )
            // Editing remains available while the daemon request is in
            // flight. Any save staged in that window still carries the old
            // path; retarget it so its later body update cannot rename the
            // document back.
            self.retargetPendingDocumentSave(sessionKey, to: newPath)
            try await self.refreshDraft(response.draftId)
            self.retargetPendingDocumentSave(sessionKey, to: newPath)
            if self.context.activeProjectId == sessionKey.projectId {
                self.didSaveDocument.send(resource?.id ?? plan.targetId)
            }
        }
    }

    private func retargetPendingDocumentSave(
        _ key: MemoryDocumentSessionKey,
        to newPath: String
    ) {
        guard let pending = pendingDocumentSaves[key] else { return }
        pendingDocumentSaves[key] = .init(
            item: pending.item,
            document: Self.documentByRetargetingPendingSave(
                pending.document,
                to: newPath
            ),
            generation: pending.generation
        )
    }

    static func documentRenamePlan(
        for item: MemoryListItem,
        currentDraft: LocalDraft?,
        newPath: String
    ) -> DocumentRenamePlan? {
        let targetId = item.resource?.id
            ?? currentDraft?.targetId
            ?? (item.resource == nil ? currentDraft?.id : nil)
        guard let targetId else { return nil }
        return .init(targetId: targetId, newPath: newPath)
    }

    static func documentByRetargetingPendingSave(
        _ document: EditableMemoryDocument,
        to newPath: String
    ) -> EditableMemoryDocument {
        var retargeted = document
        retargeted.path = newPath
        return retargeted
    }

    @discardableResult
    func delete(_ item: MemoryListItem) async -> Bool {
        guard item.draft?.isDeletion != true else { return false }
        guard !context.isSwitchingMemoryContext else {
            feedback.errorMessage = String(localized: "Wait for the Memory context switch to finish before deleting.")
            return false
        }
        guard canEditMemory(item) else {
            feedback.errorMessage = String(localized: "You do not have permission to delete this memory.")
            return false
        }
        guard sessions.synchronizationItemId(for: item) == nil else {
            feedback.errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
            return false
        }
        cancelDocumentSave(item)
        guard let projectId = draftCarrierProjectId(for: item, currentDraft: item.draft),
              let targetId = item.resource?.id ?? item.draft?.targetId else { return false }
        do {
            try await withDraftMutation {
                guard self.sessions.synchronizationItemId(for: item) == nil else {
                    throw DocumentSyncError.mutationWhileSynchronizing
                }
                let draft = self.currentDraft(for: item)
                let response = try await self.storeDraft(
                    .init(
                        draftId: draft?.id,
                        baseCommitId: draft?.baseCommitId ?? item.resource?.refCommitId,
                        projectId: projectId,
                        scope: item.scope == .org ? .org : .project,
                        resource: item.kind.daemonKind,
                        op: .delete(id: targetId, description: nil),
                        source: .desktop
                    )
                )
                try await self.refreshDraft(response.draftId)
            }
            return true
        } catch {
            feedback.errorMessage = error.localizedDescription
            return false
        }
    }

    @discardableResult
    func discard(_ draft: LocalDraft) async -> Bool {
        guard sessions.synchronizationItemId(for: draft) == nil else {
            feedback.errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
            return false
        }
        cancelDocumentSave(
            .init(projectId: draft.projectId, itemId: draft.targetId ?? draft.id)
        )
        do {
            try await withDraftMutation {
                guard self.sessions.synchronizationItemId(for: draft) == nil else {
                    throw DocumentSyncError.mutationWhileSynchronizing
                }
                guard self.drafts.contains(where: { $0.id == draft.id }) else { return }
                _ = try await self.storeDraft(
                    .init(
                        draftId: draft.id,
                        baseCommitId: draft.baseCommitId,
                        projectId: draft.projectId,
                        scope: draft.scope == .org ? .org : .project,
                        resource: draft.kind.daemonKind,
                        op: .discard(id: draft.targetId ?? draft.id),
                        source: .desktop
                    )
                )
                self.drafts.removeAll { $0.id == draft.id }
                self.documentsChanged.send()
                self.didDiscardDocument.send()
            }
            return true
        } catch {
            feedback.errorMessage = error.localizedDescription
            return false
        }
    }

    func installSynchronizedDraft(_ draft: LocalDraft) {
        if let index = drafts.firstIndex(where: { $0.id == draft.id }) {
            guard drafts[index].serverVersion <= draft.serverVersion else { return }
            drafts[index] = draft
        } else {
            drafts.append(draft)
        }
    }

    func currentDraft(for item: MemoryListItem) -> LocalDraft? {
        if let draftId = item.draft?.id,
           let draft = drafts.first(where: { $0.id == draftId }) {
            return draft
        }
        guard let resourceId = item.resource?.id else { return item.draft }
        let projectContext = item.projectContextId
            ?? (item.scope == .project ? item.projectId : nil)
        guard let projectContext else { return nil }
        return drafts.first { draft in
            draft.targetId == resourceId
                && draft.status != .discarded
                && draft.status != .merged
                && draft.projectId == projectContext
        }
    }

    private func draftCarrierProjectId(
        for item: MemoryListItem,
        currentDraft: LocalDraft?
    ) -> String? {
        currentDraft?.projectId
            ?? item.draft?.projectId
            ?? item.resource?.projectId
            ?? item.projectContextId
            ?? context.activeProjectId
    }

    func withDraftMutation<T>(_ operation: () async throws -> T) async throws -> T {
        let authority = context.authorityGeneration
        await draftMutationGate.lock()
        do {
            try context.ensureAuthority(authority)
            let result = try await operation()
            await draftMutationGate.unlock()
            return result
        } catch {
            await draftMutationGate.unlock()
            throw error
        }
    }

    private func storeDraft(_ request: DaemonDraftOperationRequest) async throws -> DaemonDraftOperationResponse {
        let authority = context.authorityGeneration
        let response = try await writeDraft(request)
        try context.ensureAuthority(authority)
        return response
    }

    private func persistDocumentSave(
        _ key: MemoryDocumentSessionKey,
        generation: UUID
    ) async {
        guard let pending = pendingDocumentSaves[key], pending.generation == generation else { return }
        guard sessions.synchronizationItemId(for: pending.item) == nil else {
            documentSaveTasks[key] = nil
            return
        }
        do {
            try await save(
                pending.item,
                document: pending.document,
                pendingSaveKey: key,
                pendingSaveGeneration: generation
            )
        } catch is CancellationError {
            return
        } catch {
            if pendingDocumentSaves[key]?.generation == generation {
                documentSaveTasks[key] = nil
                feedback.errorMessage = error.localizedDescription
            }
        }
    }

    func refreshDraft(_ draftId: String) async throws {
        let detail = try await context.daemon.draft(draftId)
        let mapped = WorkspaceLoader.mapDraft(detail, resources: catalog.resources)
        if let index = drafts.firstIndex(where: { $0.id == mapped.id }) {
            drafts[index] = mapped
        } else {
            drafts.append(mapped)
        }
        documentsChanged.send()
    }

    func remapDrafts(
        projectId: String,
        workspaceGeneration: UUID
    ) async throws {
        let projectDrafts = drafts.filter { $0.projectId == projectId }
        let originalById = Dictionary(
            projectDrafts.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        let targetIds = Set(projectDrafts.compactMap(\.targetId))
        let baselines = catalog.resources.filter { targetIds.contains($0.id) && !$0.contentLoaded }
        let loader = WorkspaceLoader(daemon: context.daemon, bootstrap: context.bootstrap, server: context.server)
        let loadedBaselines = try await concurrentMap(baselines) { try await loader.loadContent(for: $0) }
        guard context.workspaceReloadGeneration == workspaceGeneration else { return }
        for loaded in loadedBaselines {
            catalog.installLoadedResourceIfCurrent(loaded)
        }
        let resourceSnapshot = catalog.resources
        let mapped = try await concurrentMap(projectDrafts) { draft in
            WorkspaceLoader.mapDraft(
                try await self.context.daemon.draft(draft.id),
                resources: resourceSnapshot
            )
        }
        guard context.workspaceReloadGeneration == workspaceGeneration else { return }
        for candidate in mapped {
            guard let original = originalById[candidate.id],
                  let index = drafts.firstIndex(where: { $0.id == candidate.id }),
                  drafts[index] == original else { continue }
            drafts[index] = candidate
            documentsChanged.send()
        }
    }

    nonisolated static func draftInventoryPlan(
        summaries: [DaemonDraftSummary],
        currentDrafts: [LocalDraft],
        includeFailed: Bool
    ) -> DraftInventoryPlan {
        let currentById = Dictionary(
            currentDrafts.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        var refreshIds = Set<String>()
        var terminalIds = Set<String>()

        for summary in summaries {
            guard summary.status == .open || summary.status == .submitted else {
                terminalIds.insert(summary.draftId)
                continue
            }
            guard let current = currentById[summary.draftId] else {
                refreshIds.insert(summary.draftId)
                continue
            }
            let summaryChanged = current.projectId != summary.projectId
                || current.serverId != summary.serverDraftId
                || current.serverVersion != summary.serverVersion
                || current.baseCommitId != summary.baseCommitId
                || current.currentCommitId != summary.currentCommitId
                || current.freshness != summary.freshness
                || current.hasUpstreamResourceChanges != summary.hasUpstreamResourceChanges
                || current.reconciliation != summary.reconciliation
                || current.reconciliationCandidateId != summary.reconciliationCandidateId
                || current.scope != (summary.scope == .org ? .org : .project)
                || current.kind.daemonKind != summary.resourceKind
                || current.targetId != summary.targetId
                || current.status != summary.status
                || current.updatedAt != summary.updatedAt
                || (summary.path != nil && current.document.path != summary.path)
            let syncUnsettled = [.queued, .syncing, .retrying].contains(current.syncStatus)
                || (includeFailed && current.syncStatus == .failed)
            if summaryChanged || syncUnsettled {
                refreshIds.insert(summary.draftId)
            }
        }

        return .init(refreshIds: refreshIds, terminalIds: terminalIds)
    }

    func refreshDraftInventory(
        includeFailed: Bool,
        generation: UUID
    ) async {
        guard context.workspaceReloadGeneration == generation,
              context.phase == .ready,
              !Task.isCancelled else {
            return
        }

        let inventory: [DaemonDraftSummary]
        do {
            inventory = try await WorkspaceLoader.listAllDraftSummaries { query in
                try await self.context.daemon.listDrafts(query)
            }
        } catch is CancellationError {
            return
        } catch {
            guard context.workspaceReloadGeneration == generation, context.phase == .ready else { return }
            draftInventoryLoadState = .failed(
                String(localized: "Couldn’t refresh Drafts. \(error.localizedDescription)")
            )
            return
        }

        guard context.workspaceReloadGeneration == generation, context.phase == .ready, !Task.isCancelled else {
            return
        }
        let plan = Self.draftInventoryPlan(
            summaries: inventory,
            currentDrafts: drafts,
            includeFailed: includeFailed
        )
        let pendingDraftIds = Set(drafts.compactMap { draft in
            let itemId = draft.targetId ?? draft.id
            if self.pendingDocumentSaves[
                .init(projectId: draft.projectId, itemId: itemId)
            ] != nil {
                return draft.id
            }
            return nil
        })
        drafts.removeAll {
            plan.terminalIds.contains($0.id) && !pendingDraftIds.contains($0.id)
        }
        documentsChanged.send()

        let refreshIds = plan.refreshIds.subtracting(pendingDraftIds)
        guard !refreshIds.isEmpty else {
            draftInventoryLoadState = .loaded
            return
        }
        let summaries = inventory.filter { refreshIds.contains($0.draftId) }
        let originalById = Dictionary(
            drafts.filter { refreshIds.contains($0.id) }.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        let targetIds = Set(summaries.compactMap(\.targetId))
        let baselines = catalog.resources.filter { targetIds.contains($0.id) && !$0.contentLoaded }
        let loader = WorkspaceLoader(daemon: context.daemon, bootstrap: context.bootstrap, server: context.server)

        do {
            let loadedBaselines = try await concurrentMap(baselines) {
                try await loader.loadContent(for: $0)
            }
            guard context.workspaceReloadGeneration == generation,
                  context.phase == .ready,
                  !Task.isCancelled else {
                return
            }
            for loaded in loadedBaselines {
                catalog.installLoadedResourceIfCurrent(loaded)
            }
        } catch is CancellationError {
            return
        } catch {
            guard context.workspaceReloadGeneration == generation, context.phase == .ready else { return }
            draftInventoryLoadState = .failed(
                String(localized: "Couldn’t refresh Draft source files. \(error.localizedDescription)")
            )
            return
        }

        let resourceSnapshot = catalog.resources
        let mappedDrafts: [LocalDraft]
        do {
            mappedDrafts = try await concurrentMap(summaries) { summary in
                WorkspaceLoader.mapDraft(
                    try await self.context.daemon.draft(summary.draftId),
                    resources: resourceSnapshot
                )
            }
        } catch is CancellationError {
            return
        } catch {
            guard context.workspaceReloadGeneration == generation, context.phase == .ready else { return }
            draftInventoryLoadState = .failed(
                String(localized: "Couldn’t refresh Draft details. \(error.localizedDescription)")
            )
            return
        }

        guard context.workspaceReloadGeneration == generation, context.phase == .ready, !Task.isCancelled else {
            return
        }
        for mapped in mappedDrafts {
            if let index = drafts.firstIndex(where: { $0.id == mapped.id }) {
                guard originalById[mapped.id] == drafts[index] else { continue }
                drafts[index] = mapped
            } else if originalById[mapped.id] == nil {
                drafts.append(mapped)
            }
            documentsChanged.send()
        }
        draftInventoryLoadState = .loaded
    }

    func daemonContent(kind: MemoryKind, document: EditableMemoryDocument) -> DaemonDraftContent {
        .init(description: nil, content: document.body)
    }

    private func validatePath(kind: MemoryKind, path: String) throws {
        let segments = path.split(separator: "/", omittingEmptySubsequences: false)
        if path.isEmpty
            || path.hasPrefix("/")
            || path.hasSuffix("/")
            || segments.contains(where: { $0.isEmpty || $0 == "." || $0 == ".." }) {
            throw MemoryValidationError.invalidPath(String(localized: "Use a normalized relative path with / separators."))
        }
        if kind == .workflows && !path.hasPrefix("workflow/") {
            throw MemoryValidationError.invalidPath(String(localized: "Workflow paths must use the workflow/ namespace."))
        }
        if kind == .rules && path.lowercased().hasPrefix("workflow/") {
            throw MemoryValidationError.invalidPath(String(localized: "Rule paths cannot use the workflow/ namespace."))
        }
    }

    func validate(kind: MemoryKind, document: EditableMemoryDocument) throws {
        try validatePath(kind: kind, path: document.path)
        if kind == .rules && document.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            throw MemoryValidationError.emptyRule
        }
    }

    func startLoading(
        generation: UUID,
        requiresFreshData: Bool,
        baseSnapshotWasStale: Bool
    ) {
        cancelLoading()
        let loader = context.loader
        let resourcesAtReady = catalog.resources
        let accessibleProjectIds = Set(context.projects.map(\.id))
        let baselineDrafts = drafts
        draftInventoryLoadState = .loading
        draftInventoryLoadTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.context.workspaceReloadGeneration == generation {
                    self.draftInventoryLoadTask = nil
                }
            }
            do {
                let loaded = try await loader.loadDeferredDrafts(
                    resources: resourcesAtReady,
                    accessibleProjectIds: accessibleProjectIds
                )
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
                    draftInventoryLoadState = .failed(
                        String(localized: "Fresh Draft data was unavailable. Existing Drafts were kept.")
                    )
                    return
                }
                for resource in loaded.loadedBaselines {
                    catalog.installLoadedResourceIfCurrent(resource)
                }
                drafts = WorkspaceLoadPolicy.mergeDeferredRecords(
                    baseline: baselineDrafts,
                    current: drafts,
                    loaded: loaded.drafts
                )
                draftInventoryLoadState = .loaded
                documentsChanged.send()
            } catch is CancellationError {
                return
            } catch {
                guard let self, context.workspaceReloadGeneration == generation else { return }
                draftInventoryLoadState = .failed(error.localizedDescription)
            }
        }
    }

    func cancelLoading() {
        draftInventoryLoadTask?.cancel()
        draftInventoryLoadTask = nil
    }

    var hasPendingChanges: Bool { !pendingDocumentSaves.isEmpty }

    func resetAuthority() {
        cancelLoading()
        documentSaveTasks.values.forEach { $0.cancel() }
        documentSaveTasks.removeAll()
        pendingDocumentSaves.removeAll()
        drafts.removeAll()
        draftInventoryLoadState = .loading
    }

    func retainAccessibleProjects(_ projectIds: Set<String>) {
        drafts = WorkspaceLoadPolicy.retainingAccessibleProjectRecords(
            drafts, accessibleProjectIds: projectIds, projectId: \.projectId
        )
    }
}

enum MemoryValidationError: LocalizedError, Sendable {
    case invalidPath(String)
    case emptyRule
    case memoryCannotBeRenamed

    var errorDescription: String? {
        switch self {
        case .invalidPath(let message): message
        case .emptyRule: String(localized: "A Rule needs content.")
        case .memoryCannotBeRenamed:
            String(localized: "This memory is no longer available to rename.")
        }
    }
}

struct DocumentRenamePlan: Equatable, Sendable {
    let targetId: String
    let newPath: String
}

struct DraftInventoryPlan: Equatable, Sendable {
    let refreshIds: Set<String>
    let terminalIds: Set<String>
}
