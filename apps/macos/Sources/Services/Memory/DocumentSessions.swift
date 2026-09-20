import Combine
import Foundation

@MainActor
final class DocumentSessions: ObservableObject {
    private let context: WorkspaceContext
    let didFinish = PassthroughSubject<MemoryDocumentSessionKey, Never>()

    init(context: WorkspaceContext) {
        self.context = context
    }

    @Published var synchronizingDocumentSessions:
        Set<MemoryDocumentSessionKey> = []

    @Published var pendingDocumentReconciliationCandidatesBySession:
        [MemoryDocumentSessionKey: DraftReconciliationCandidate] = [:]

    @Published var applyingDocumentReconciliationSessions:
        Set<MemoryDocumentSessionKey> = []

    @Published var projectOrgSelectionMutatingIds: Set<String> = []

    var documentSynchronizationGenerations: [MemoryDocumentSessionKey: UUID] = [:]

    var documentSynchronizationTasks: [MemoryDocumentSessionKey: Task<Void, Never>] = [:]

    var documentReconciliationResolutions:
        [MemoryDocumentSessionKey: DraftResolution] = [:]

    /// Review reconciliation has no open document session, but it still must
    /// exclude a daemon Project-context switch while a candidate/rebase is in flight.
    var standaloneReconciliationActivityIds: Set<UUID> = []

    /// UI compatibility view of reconciliation state for the active Project.
    /// The stored state remains context-scoped so an equal Org resource id in
    /// another Project can never be rendered or applied here.
    var pendingDocumentReconciliationCandidates: [String: DraftReconciliationCandidate] {
        guard let activeProjectId = context.activeProjectId else { return [:] }
        return Dictionary(
            uniqueKeysWithValues: pendingDocumentReconciliationCandidatesBySession.compactMap {
                key, candidate in
                key.projectId == activeProjectId ? (key.itemId, candidate) : nil
            }
        )
    }

    func isSynchronizingDocument(_ itemId: String) -> Bool {
        guard let key = activeDocumentSessionKey(for: itemId) else { return false }
        return synchronizingDocumentSessions.contains(key)
    }

    func documentReconciliationResolution(for itemId: String) -> DraftResolution? {
        guard let key = activeDocumentSessionKey(for: itemId) else { return nil }
        return documentReconciliationResolutions[key]
    }

    func updateDocumentReconciliationResolution(
        _ resolution: DraftResolution,
        for itemId: String
    ) {
        guard let key = activeDocumentSessionKey(for: itemId),
              pendingDocumentReconciliationCandidatesBySession[key] != nil else { return }
        documentReconciliationResolutions[key] = resolution
    }

    func finishDocumentReconciliation(for key: MemoryDocumentSessionKey) {
        guard key.projectId == context.activeProjectId,
              !applyingDocumentReconciliationSessions.contains(key) else { return }
        documentSynchronizationTasks.removeValue(forKey: key)?.cancel()
        pendingDocumentReconciliationCandidatesBySession.removeValue(forKey: key)
        documentReconciliationResolutions.removeValue(forKey: key)
        synchronizingDocumentSessions.remove(key)
        documentSynchronizationGenerations.removeValue(forKey: key)
        didFinish.send(key)
    }

    func activeDocumentSessionKey(for itemId: String) -> MemoryDocumentSessionKey? {
        context.activeProjectId.map { MemoryDocumentSessionKey(projectId: $0, itemId: itemId) }
    }

    func documentSessionKey(for item: MemoryListItem) -> MemoryDocumentSessionKey? {
        Self.memoryDocumentSessionKey(for: item)
    }

    nonisolated static func memoryDocumentSessionKey(
        for item: MemoryListItem
    ) -> MemoryDocumentSessionKey? {
        let projectId = item.projectContextId
            ?? (item.scope == .project ? item.projectId : nil)
        return projectId.map { MemoryDocumentSessionKey(projectId: $0, itemId: item.id) }
    }

    func documentSessionKey(for tab: WorkbenchTab) -> MemoryDocumentSessionKey? {
        tab.projectId.map { MemoryDocumentSessionKey(projectId: $0, itemId: tab.itemId) }
    }

    func synchronizationItemId(for item: MemoryListItem) -> String? {
        guard let projectId = documentSessionKey(for: item)?.projectId else { return nil }
        return [item.id, item.resource?.id, item.draft?.targetId, item.draft?.id]
            .compactMap { $0 }
            .first {
                self.synchronizingDocumentSessions.contains(
                    .init(projectId: projectId, itemId: $0)
                )
            }
    }

    func synchronizationItemId(for draft: LocalDraft) -> String? {
        [draft.targetId, draft.id]
            .compactMap { $0 }
            .first {
                self.synchronizingDocumentSessions.contains(
                    .init(projectId: draft.projectId, itemId: $0)
                )
            }
    }

    func hasDocumentSynchronization(in projectId: String) -> Bool {
        synchronizingDocumentSessions.contains { $0.projectId == projectId }
    }

    func isCurrentDocumentSynchronization(
        _ key: MemoryDocumentSessionKey,
        generation: UUID
    ) -> Bool {
        context.activeProjectId == key.projectId
            && !context.isSwitchingMemoryContext
            && synchronizingDocumentSessions.contains(key)
            && documentSynchronizationGenerations[key] == generation
    }

    func endDocumentSynchronization(
        _ key: MemoryDocumentSessionKey,
        generation: UUID
    ) {
        guard documentSynchronizationGenerations[key] == generation else { return }
        documentSynchronizationTasks.removeValue(forKey: key)
        synchronizingDocumentSessions.remove(key)
        documentSynchronizationGenerations.removeValue(forKey: key)
    }

    nonisolated static func canStartDocumentSynchronization(
        isSwitchingMemoryContext: Bool,
        activeProjectId: String?,
        itemProjectContextId: String?
    ) -> Bool {
        guard let itemProjectContextId else { return false }
        return WorkspaceContext.projectContextIsCurrent(
            isSwitchingMemoryContext: isSwitchingMemoryContext,
            activeProjectId: activeProjectId,
            expectedProjectId: itemProjectContextId
        )
    }

    nonisolated static func canCommitMemoryContextSwitch(
        hasDocumentSynchronization: Bool,
        hasApplyingDocumentReconciliation: Bool,
        hasStandaloneReconciliationActivity: Bool
    ) -> Bool {
        !hasDocumentSynchronization
            && !hasApplyingDocumentReconciliation
            && !hasStandaloneReconciliationActivity
    }

    var canCommitMemoryContextSwitch: Bool {
        Self.canCommitMemoryContextSwitch(
            hasDocumentSynchronization: !synchronizingDocumentSessions.isEmpty,
            hasApplyingDocumentReconciliation: !applyingDocumentReconciliationSessions.isEmpty,
            hasStandaloneReconciliationActivity: !standaloneReconciliationActivityIds.isEmpty
        )
    }

    func clearDocumentSynchronizationState(for tab: WorkbenchTab) {
        guard let key = documentSessionKey(for: tab) else { return }
        documentSynchronizationTasks.removeValue(forKey: key)?.cancel()
        pendingDocumentReconciliationCandidatesBySession.removeValue(forKey: key)
        documentReconciliationResolutions.removeValue(forKey: key)
        synchronizingDocumentSessions.remove(key)
        documentSynchronizationGenerations.removeValue(forKey: key)
    }

    func resetDocumentTasks() {
        documentSynchronizationTasks.values.forEach { $0.cancel() }
        documentSynchronizationTasks.removeAll()
        pendingDocumentReconciliationCandidatesBySession.removeAll()
        documentReconciliationResolutions.removeAll()
        synchronizingDocumentSessions.removeAll()
        documentSynchronizationGenerations.removeAll()
    }

    func resetAuthority() {
        resetDocumentTasks()
        projectOrgSelectionMutatingIds.removeAll()
        applyingDocumentReconciliationSessions.removeAll()
        standaloneReconciliationActivityIds.removeAll()
    }
}
