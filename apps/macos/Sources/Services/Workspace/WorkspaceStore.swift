import AppKit
import Combine
import CryptoKit
import Foundation
import UniformTypeIdentifiers

private struct SyncRetryTaskHandle {
    let id: UUID
    let task: Task<SyncRetryOutcome, Never>
}

private enum WorkspaceBackgroundErrorSource: Hashable {
    case organizationResources
    case staleResources(projectId: String)
    case projectRefresh(projectId: String)
}

private struct WorkspaceBackgroundErrorPresentation {
    let source: WorkspaceBackgroundErrorSource
    let message: String
}

private struct ResourceLoadRequest: Sendable {
    let resource: MemoryResource
    let generation: UUID
    let task: Task<String?, Never>
}

@MainActor
final class WorkspaceStore: ObservableObject {
    @Published private(set) var phase: ApplicationPhase = .launching
    @Published private(set) var account: UserReference?
    @Published private(set) var organization: OrganizationReference?
    @Published private(set) var capabilities: Set<String> = []
    @Published private(set) var projects: [ProjectState] = []
    @Published private(set) var projectRoles: [String: ProjectMemberRole] = [:]
    @Published private(set) var projectBindingsGeneration = UUID()
    @Published private(set) var isMutatingAdministration = false
    @Published private(set) var authorityGeneration = UUID()
    let projectDirectoryChanges = PassthroughSubject<Void, Never>()

    @Published private(set) var orgRefCommitId: String?
    @Published private(set) var orgRefEtag = ""
    @Published private(set) var resources: [MemoryResource] = []
    @Published private(set) var drafts: [LocalDraft] = []
    @Published private(set) var bundles: [PersonalBundle] = []
    @Published private(set) var reviews: [ReviewRecord] = []
    @Published private(set) var draftInventoryLoadState: WorkspaceCollectionLoadState = .loading
    @Published private(set) var bundleLoadState: WorkspaceCollectionLoadState = .loading
    @Published private(set) var reviewLoadState: WorkspaceCollectionLoadState = .loading
    @Published private(set) var runtime: RuntimeState?
    @Published private(set) var syncStatusAvailable = true
    @Published private var retryingSyncKeys: Set<SyncRetryKey> = []
    @Published private var syncRetryErrors: [SyncRetryKey: String] = [:]
    @Published private(set) var legacyAgentAdapterConflicts: [DaemonLegacyAgentAdapterConflict] = []
    @Published private(set) var legacyAgentAdapterInspectionWarning: String?

    @Published var activeProjectId: String? = nil {
        didSet {
            guard oldValue != activeProjectId else { return }
            clearIrrelevantScopedErrorPresentation()
        }
    }
    @Published var selectedSection: WorkspaceSection = .memory
    @Published var selectedKind: MemoryKind = .context
    @Published var selectedItemId: String?
    @Published var selectedBundleId: String?
    @Published var selectedReviewId: String?
    @Published var searchQuery = ""
    @Published var workspaceSearchFocusToken = UUID()
    @Published var reviewSearchFocusToken = UUID()
    @Published var showsProjectCreation = false
    @Published var showsProjectSettings = false
    @Published var sidebarExpanded = true
    @Published var errorMessage: String?
    @Published var tabs: [WorkbenchTab] = []
    @Published var activeTabId: String?
    @Published private(set) var navigationBackStack: [String] = []
    @Published private(set) var navigationForwardStack: [String] = []
    @Published var pendingDocumentCommand: DocumentSessionCommand?
    @Published var pendingReviewReconciliationId: String?
    @Published var reviewDecisionReadiness: ReviewDecisionReadiness?
    @Published var documentReconciliationToolbarState: DocumentReconciliationToolbarState?
    @Published private(set) var loadingResourceIds: Set<String> = []
    @Published private(set) var loadingProjectId: String?
    @Published private(set) var isSwitchingMemoryContext = false
    @Published private(set) var isPreparingWorkspaceIndex = false
    @Published private(set) var isExportingMemory = false
    /// Project resources whose shared version moved forward after the app
    /// loaded its snapshot; they show a sync icon and can be refreshed.
    @Published private(set) var staleResourceIds: Set<String> = []
    /// Incremented when authoritative content replaces the document currently
    /// held by a long-lived editor session.
    @Published private(set) var documentContentGenerations: [String: UInt64] = [:]
    @Published private(set) var synchronizingDocumentSessions:
        Set<MemoryDocumentSessionKey> = []
    @Published private var pendingDocumentReconciliationCandidatesBySession:
        [MemoryDocumentSessionKey: DraftReconciliationCandidate] = [:]
    @Published private var applyingDocumentReconciliationSessions:
        Set<MemoryDocumentSessionKey> = []
    @Published private(set) var projectOrgSelectionMutatingIds: Set<String> = []

    let daemon = DaemonXPCClient()
    private let bootstrap = DaemonBootstrapController()
    private(set) lazy var server = ServerClient(daemon: daemon)
    private(set) var workspaceReloadGeneration = UUID()
    private var administrationMutationGeneration = UUID()
    private var draftInventoryLoadTask: Task<Void, Never>?
    private var bundleLoadTask: Task<Void, Never>?
    private var reviewLoadTask: Task<Void, Never>?
    private var legacyAgentAdapterInspectionTask: Task<Void, Never>?
    private var postReadyRetrySyncTask: Task<Void, Never>?
    private var isRefreshingSyncStatus = false
    private var isRefreshingSynchronizedWorkspaceData = false
    private var syncRetryTasks: [SyncRetryKey: SyncRetryTaskHandle] = [:]
    private var presentedSyncRetryErrorKey: SyncRetryKey?
    private var dismissedBackgroundErrorSources: Set<WorkspaceBackgroundErrorSource> = []
    private var presentedBackgroundError: WorkspaceBackgroundErrorPresentation?
    private var isSigningOut = false
    private var projectSelectionGeneration = UUID()
    private let projectSelectionSideEffectGate = ProjectSelectionSideEffectGate()
    private let draftMutationGate = AsyncMutex()
    private let bundleMutationGate = AsyncMutex()
    private let projectOrgSelectionMutationGate = AsyncMutex()
    private var pendingDocumentSaves: [MemoryDocumentSessionKey: PendingDocumentSave] = [:]
    private var documentSaveTasks: [MemoryDocumentSessionKey: Task<Void, Never>] = [:]
    private var pendingBundleSaves: [String: PendingBundleSave] = [:]
    private var bundleSaveTasks: [String: Task<Void, Never>] = [:]
    private var staleResourceSnapshots: [String: StaleResourceSyncSnapshot] = [:]
    private var provisionalStaleAdditionIds: Set<String> = []
    private var resourceLoadRequests: [String: ResourceLoadRequest] = [:]
    private var staleResourceRefreshGenerations: [String: UUID] = [:]
    private var orgResourceRefreshGeneration: UUID?
    private var documentSynchronizationGenerations: [MemoryDocumentSessionKey: UUID] = [:]
    private var documentSynchronizationTasks: [MemoryDocumentSessionKey: Task<Void, Never>] = [:]
    private var documentReconciliationResolutions:
        [MemoryDocumentSessionKey: ReconciliationResourceState] = [:]
    /// Review reconciliation has no open document session, but it still must
    /// exclude a daemon Project-context switch while a candidate/rebase is in flight.
    private var standaloneReconciliationActivityIds: Set<UUID> = []

    var activeProject: ProjectState? {
        projects.first { $0.id == activeProjectId }
    }

    var isRetryingSync: Bool {
        retryingSyncKeys.contains { $0.projectId == activeProjectId }
    }

    var syncRetryErrorMessage: String? {
        let messages = syncRetryErrors
            .filter { $0.key.projectId == activeProjectId }
            .sorted { $0.key.channel < $1.key.channel }
            .map(\.value)
        return messages.isEmpty ? nil : messages.joined(separator: "\n")
    }

    func isRetryingSync(channel: String, projectId: String?) -> Bool {
        retryingSyncKeys.contains(
            SyncRetryKey(channel: channel, projectId: projectId)
        )
    }

    func dismissErrorMessage() {
        if let presentation = presentedBackgroundError,
           errorMessage == presentation.message {
            dismissedBackgroundErrorSources.insert(presentation.source)
        }
        errorMessage = nil
        presentedBackgroundError = nil
        presentedSyncRetryErrorKey = nil
    }

    private func presentBackgroundError(
        _ message: String,
        source: WorkspaceBackgroundErrorSource
    ) {
        guard backgroundErrorIsRelevant(source),
              !dismissedBackgroundErrorSources.contains(source),
              errorMessage == nil || errorMessage == presentedBackgroundError?.message else {
            return
        }
        presentedBackgroundError = .init(source: source, message: message)
        errorMessage = message
    }

    private func resolveBackgroundError(_ source: WorkspaceBackgroundErrorSource) {
        dismissedBackgroundErrorSources.remove(source)
        guard let presentation = presentedBackgroundError,
              presentation.source == source else {
            return
        }
        if errorMessage == presentation.message {
            errorMessage = nil
        }
        presentedBackgroundError = nil
    }

    private func backgroundErrorIsRelevant(_ source: WorkspaceBackgroundErrorSource) -> Bool {
        switch source {
        case .organizationResources:
            return selectedSection == .memory && activeProjectId == nil
        case .staleResources(let projectId), .projectRefresh(let projectId):
            return activeProjectId == projectId
        }
    }

    private func clearIrrelevantScopedErrorPresentation() {
        if let key = presentedSyncRetryErrorKey, key.projectId != activeProjectId {
            if errorMessage == syncRetryErrors[key] {
                errorMessage = nil
            }
            presentedSyncRetryErrorKey = nil
        }
        if let presentation = presentedBackgroundError,
           !backgroundErrorIsRelevant(presentation.source) {
            if errorMessage == presentation.message {
                errorMessage = nil
            }
            presentedBackgroundError = nil
        }
    }

    private func cancelSyncRetries() {
        if let key = presentedSyncRetryErrorKey,
           errorMessage == syncRetryErrors[key] {
            errorMessage = nil
        }
        syncRetryTasks.values.forEach { $0.task.cancel() }
        syncRetryTasks.removeAll()
        retryingSyncKeys.removeAll()
        syncRetryErrors.removeAll()
        presentedSyncRetryErrorKey = nil
    }

    private func clearSyncRetryErrors(channel: String, projectId: String?) {
        let errorKeysToClear = channel == "all"
            ? syncRetryErrors.keys.filter { $0.projectId == projectId }
            : [SyncRetryKey(channel: channel, projectId: projectId)]
        let presentedError = presentedSyncRetryErrorKey.flatMap { syncRetryErrors[$0] }
        if let presentedSyncRetryErrorKey,
           errorKeysToClear.contains(presentedSyncRetryErrorKey),
           errorMessage == presentedError {
            errorMessage = nil
            self.presentedSyncRetryErrorKey = nil
        }
        for errorKey in errorKeysToClear {
            syncRetryErrors[errorKey] = nil
        }
    }

    private func resetBackgroundErrorPresentation() {
        if let presentation = presentedBackgroundError,
           errorMessage == presentation.message {
            errorMessage = nil
        }
        dismissedBackgroundErrorSources.removeAll()
        presentedBackgroundError = nil
    }

    var canCreateProject: Bool {
        Self.projectCreationAllowed(capabilities: capabilities)
    }

    nonisolated static func projectCreationAllowed(capabilities: Set<String>) -> Bool {
        capabilities.contains("project:create") || capabilities.contains("admin:write")
    }

    func canManageProject(_ projectId: String) -> Bool {
        Self.projectManagementAllowed(capabilities: capabilities, role: projectRoles[projectId])
    }

    nonisolated static func projectManagementAllowed(
        capabilities: Set<String>, role: ProjectMemberRole?
    ) -> Bool {
        capabilities.contains("admin:write") || role == .admin
    }

    func canAccessProjectSettings(_ projectId: String) -> Bool {
        canAdministerOrganization || projects.contains { $0.id == projectId }
    }

    var canAdministerOrganization: Bool {
        capabilities.contains("admin:write")
    }

    var canMergeReviews: Bool {
        capabilities.contains("review:merge")
    }

    var canDecideReviews: Bool {
        capabilities.contains("review:decide")
    }

    var hasPendingChanges: Bool {
        !pendingDocumentSaves.isEmpty || !pendingBundleSaves.isEmpty
    }

    func isReviewAuthor(_ review: ReviewRecord) -> Bool {
        account?.userId == review.author.userId
    }

    func canCreateMemory(kind: MemoryKind, scope: MemoryScope) -> Bool {
        !isSwitchingMemoryContext && activeProjectId != nil && scope == .org
    }

    /// Authority is never edited in place: a Project member edits selected
    /// Organization Memory through a Project-bound LocalDraft. Legacy
    /// Project-scoped authority remains visible but read-only.
    func canEditMemory(_ item: MemoryListItem) -> Bool {
        guard phase == .ready else { return false }
        let projectContextId = item.projectContextId
            ?? (item.scope == .project ? item.projectId : nil)
        guard let projectContextId,
              projectContextId == activeProjectId else {
            return false
        }
        if projectOrgSelectionMutatingIds.contains(projectContextId) {
            return false
        }
        guard item.scope == .org else { return false }
        if let draft = item.draft, draft.targetId == nil { return true }
        return item.inherited
    }

    var selectedItem: MemoryListItem? {
        let itemId = activeVisibleTab?.itemId ?? selectedItemId
        return visibleMemoryItems.first { $0.id == itemId } ?? visibleMemoryItems.first
    }

    var visibleTabs: [WorkbenchTab] {
        tabs.filter { tab in
            guard tab.isVisible(in: selectedSection, projectId: activeProjectId) else {
                return false
            }
            guard selectedSection == .memory else { return true }
            let allowsUnresolved = phase != .ready || activeProject?.isLoaded == false
            guard let activeProjectId else {
                return Self.orgMemoryTabIsAvailable(
                    itemId: tab.itemId,
                    resources: resources,
                    allowsUnresolved: allowsUnresolved
                )
            }
            guard tab.projectId == activeProjectId else { return false }
            return Self.memoryTabIsAvailable(
                itemId: tab.itemId,
                projectId: activeProjectId,
                selectedOrgResourceIds: activeProject?.selectedOrgResourceIds ?? [],
                resources: resources,
                drafts: drafts,
                allowsUnresolved: allowsUnresolved
            )
        }
    }

    nonisolated static func orgMemoryTabIsAvailable(
        itemId: String,
        resources: [MemoryResource],
        allowsUnresolved: Bool = false
    ) -> Bool {
        if resources.contains(where: { $0.id == itemId && $0.scope == .org }) {
            return true
        }
        return allowsUnresolved
    }

    nonisolated static func memoryTabIsAvailable(
        itemId: String,
        projectId: String,
        selectedOrgResourceIds: Set<String>,
        resources: [MemoryResource],
        drafts: [LocalDraft],
        allowsUnresolved: Bool = false
    ) -> Bool {
        let resource = resources.first(where: { $0.id == itemId })
        if let resource {
            switch resource.scope {
            case .project:
                return resource.projectId == projectId
            case .org:
                if selectedOrgResourceIds.contains(resource.id) { return true }
            }
        }
        if drafts.contains(where: { draft in
            (draft.id == itemId || draft.targetId == itemId)
                && draft.projectId == projectId
                && draft.status != .discarded
                && draft.status != .merged
        }) {
            return true
        }
        // Keep unresolved tabs while a workspace generation is loading. A
        // known, unselected Org resource is intentionally hidden; a document
        // whose resource has not arrived yet must not be dropped prematurely.
        return allowsUnresolved && resource == nil
    }

    /// Resolves whether a stored tab still belongs to its own view context.
    /// A globally live Org resource is not enough to retain a clean Project
    /// tab after that Project removes the resource from its selection.
    nonisolated static func memoryTabIsAvailable(
        _ tab: WorkbenchTab,
        projects: [ProjectState],
        resources: [MemoryResource],
        drafts: [LocalDraft],
        allowsUnresolvedOrg: Bool = false
    ) -> Bool {
        guard tab.section == .memory else { return true }
        guard let projectId = tab.projectId else {
            return orgMemoryTabIsAvailable(
                itemId: tab.itemId,
                resources: resources,
                allowsUnresolved: allowsUnresolvedOrg
            )
        }
        guard let project = projects.first(where: { $0.id == projectId }) else {
            return false
        }
        return memoryTabIsAvailable(
            itemId: tab.itemId,
            projectId: projectId,
            selectedOrgResourceIds: project.selectedOrgResourceIds,
            resources: resources,
            drafts: drafts,
            allowsUnresolved: !project.isLoaded
        )
    }

    nonisolated static func retainedMemoryTabs(
        _ tabs: [WorkbenchTab],
        projects: [ProjectState],
        resources: [MemoryResource],
        drafts: [LocalDraft],
        allowsUnresolvedOrg: Bool = false
    ) -> [WorkbenchTab] {
        tabs.filter {
            memoryTabIsAvailable(
                $0,
                projects: projects,
                resources: resources,
                drafts: drafts,
                allowsUnresolvedOrg: allowsUnresolvedOrg
            )
        }
    }

    var activeVisibleTab: WorkbenchTab? {
        visibleTabs.first { $0.id == activeTabId } ?? visibleTabs.last
    }

    var canGoBack: Bool {
        navigationBackStack.contains(where: isVisibleTab)
    }

    var canGoForward: Bool {
        navigationForwardStack.contains(where: isVisibleTab)
    }

    var currentItem: MemoryListItem? {
        guard let tab = activeVisibleTab else { return nil }
        return item(for: tab)
    }

    var currentTabMode: WorkbenchTabMode? {
        activeVisibleTab?.mode
    }

    /// UI compatibility view of reconciliation state for the active Project.
    /// The stored state remains context-scoped so an equal Org resource id in
    /// another Project can never be rendered or applied here.
    var pendingDocumentReconciliationCandidates: [String: DraftReconciliationCandidate] {
        guard let activeProjectId else { return [:] }
        return Dictionary(
            uniqueKeysWithValues: pendingDocumentReconciliationCandidatesBySession.compactMap {
                key, candidate in
                key.projectId == activeProjectId ? (key.itemId, candidate) : nil
            }
        )
    }

    func documentContentGeneration(for itemId: String) -> UInt64 {
        documentContentGenerations[itemId, default: 0]
    }

    func staleResourceGeneration(for resourceId: String) -> UUID? {
        guard let activeProjectId,
              let snapshot = staleResourceSnapshots[resourceId],
              snapshot.projectId == activeProjectId else { return nil }
        return snapshot.generation
    }

    func isSynchronizingDocument(_ itemId: String) -> Bool {
        guard let key = activeDocumentSessionKey(for: itemId) else { return false }
        return synchronizingDocumentSessions.contains(key)
    }

    func pendingDocument(for item: MemoryListItem) -> EditableMemoryDocument? {
        guard let key = documentSessionKey(for: item) else { return nil }
        return pendingDocumentSaves[key]?.document
    }

    func documentReconciliationResolution(for itemId: String) -> ReconciliationResourceState? {
        guard let key = activeDocumentSessionKey(for: itemId) else { return nil }
        return documentReconciliationResolutions[key]
    }

    func updateDocumentReconciliationResolution(
        _ resolution: ReconciliationResourceState,
        for itemId: String
    ) {
        guard let key = activeDocumentSessionKey(for: itemId),
              pendingDocumentReconciliationCandidatesBySession[key] != nil else { return }
        documentReconciliationResolutions[key] = resolution
    }

    func finishDocumentReconciliation(for key: MemoryDocumentSessionKey) {
        guard key.projectId == activeProjectId,
              !applyingDocumentReconciliationSessions.contains(key) else { return }
        documentSynchronizationTasks.removeValue(forKey: key)?.cancel()
        pendingDocumentReconciliationCandidatesBySession.removeValue(forKey: key)
        documentReconciliationResolutions.removeValue(forKey: key)
        synchronizingDocumentSessions.remove(key)
        documentSynchronizationGenerations.removeValue(forKey: key)
        if documentReconciliationToolbarState?.sessionKey == key {
            documentReconciliationToolbarState = nil
        }
        if pendingDocumentCommand?.sessionKey == key {
            pendingDocumentCommand = nil
        }
    }

    private func activeDocumentSessionKey(for itemId: String) -> MemoryDocumentSessionKey? {
        activeProjectId.map { MemoryDocumentSessionKey(projectId: $0, itemId: itemId) }
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

    private func documentSessionKey(for tab: WorkbenchTab) -> MemoryDocumentSessionKey? {
        tab.projectId.map { MemoryDocumentSessionKey(projectId: $0, itemId: tab.itemId) }
    }

    private func synchronizationItemId(for item: MemoryListItem) -> String? {
        guard let projectId = documentSessionKey(for: item)?.projectId else { return nil }
        return [item.id, item.resource?.id, item.draft?.targetId, item.draft?.id]
            .compactMap { $0 }
            .first {
                synchronizingDocumentSessions.contains(
                    .init(projectId: projectId, itemId: $0)
                )
            }
    }

    private func synchronizationItemId(for draft: LocalDraft) -> String? {
        [draft.targetId, draft.id]
            .compactMap { $0 }
            .first {
                synchronizingDocumentSessions.contains(
                    .init(projectId: draft.projectId, itemId: $0)
                )
            }
    }

    private func hasDocumentSynchronization(in projectId: String) -> Bool {
        synchronizingDocumentSessions.contains { $0.projectId == projectId }
    }

    private func isCurrentDocumentSynchronization(
        _ key: MemoryDocumentSessionKey,
        generation: UUID
    ) -> Bool {
        activeProjectId == key.projectId
            && !isSwitchingMemoryContext
            && synchronizingDocumentSessions.contains(key)
            && documentSynchronizationGenerations[key] == generation
    }

    private func endDocumentSynchronization(
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
        return projectContextIsCurrent(
            isSwitchingMemoryContext: isSwitchingMemoryContext,
            activeProjectId: activeProjectId,
            expectedProjectId: itemProjectContextId
        )
    }

    nonisolated static func projectContextIsCurrent(
        isSwitchingMemoryContext: Bool,
        activeProjectId: String?,
        expectedProjectId: String
    ) -> Bool {
        !isSwitchingMemoryContext && activeProjectId == expectedProjectId
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

    private var canCommitMemoryContextSwitch: Bool {
        Self.canCommitMemoryContextSwitch(
            hasDocumentSynchronization: !synchronizingDocumentSessions.isEmpty,
            hasApplyingDocumentReconciliation: !applyingDocumentReconciliationSessions.isEmpty,
            hasStandaloneReconciliationActivity: !standaloneReconciliationActivityIds.isEmpty
        )
    }

    private func clearPendingDocumentSessionPresentation() {
        pendingDocumentCommand = nil
        documentReconciliationToolbarState = nil
    }

    func documentPathChanges(for item: MemoryListItem) -> [DocumentPathChange] {
        if let draft = item.draft {
            // The mapped document for a behind draft is based on the currently
            // displayed resource and cannot attribute a remote rename. The
            // candidate owns the exact base/current/draft paths instead.
            guard draft.freshness != .behind else { return [] }
            let basePath = item.resource?.document.path
            return Self.documentPathChanges(
                basePath: basePath,
                localPath: draft.isDeletion ? nil : draft.document.path,
                remotePath: basePath
            )
        }
        guard let resource = item.resource,
              let snapshot = staleResourceSnapshot(for: item),
              snapshot.local?.id == resource.id || snapshot.remote?.id == resource.id else {
            return []
        }
        let basePath = snapshot.local?.document.path
        return Self.documentPathChanges(
            basePath: basePath,
            localPath: basePath,
            remotePath: snapshot.remote?.document.path
        )
    }

    private func staleResourceSnapshot(
        for item: MemoryListItem
    ) -> StaleResourceSyncSnapshot? {
        guard let resourceId = item.resource?.id,
              let projectId = item.projectContextId,
              projectId == activeProjectId,
              let snapshot = staleResourceSnapshots[resourceId],
              snapshot.projectId == projectId else { return nil }
        return snapshot
    }

    nonisolated static func documentPathChanges(
        basePath: String?,
        localPath: String?,
        remotePath: String?
    ) -> [DocumentPathChange] {
        let hasLocalChange = localPath != basePath
        let hasRemoteChange = remotePath != basePath
        if hasLocalChange, hasRemoteChange, localPath == remotePath {
            return [.init(source: .draftAndShared, from: basePath, to: localPath)]
        }
        var changes: [DocumentPathChange] = []
        if hasLocalChange {
            changes.append(.init(source: .draft, from: basePath, to: localPath))
        }
        if hasRemoteChange {
            changes.append(.init(source: .shared, from: basePath, to: remotePath))
        }
        return changes
    }

    var selectedBundle: PersonalBundle? {
        bundles.first { $0.id == selectedBundleId } ?? bundles.first
    }

    var selectedReview: ReviewRecord? {
        reviews.first { $0.id == selectedReviewId } ?? reviews.first
    }

    func review(for draft: LocalDraft) -> ReviewRecord? {
        guard draft.status == .submitted, let serverId = draft.serverId else { return nil }
        return reviews.first {
            $0.projectId == draft.projectId
                && ($0.status == "open" || $0.status == "approved")
                && ($0.draftId == serverId || $0.draftIds.contains(serverId))
        }
    }

    func openReview(_ review: ReviewRecord) {
        selectedReviewId = review.id
        selectedSection = .reviews
    }

    func openReview(for draft: LocalDraft) async {
        let generation = workspaceReloadGeneration
        let projectId = activeProjectId
        let section = selectedSection
        do {
            if review(for: draft) == nil {
                let baseline = reviews
                let loaded = try await WorkspaceLoader(
                    daemon: daemon, bootstrap: bootstrap, server: server
                ).loadReviews()
                guard workspaceReloadGeneration == generation,
                      activeProjectId == projectId,
                      selectedSection == section, !Task.isCancelled else { return }
                reviews = Self.mergeDeferredRecords(
                    baseline: baseline, current: reviews, loaded: loaded.records
                )
                reviewLoadState = .loaded
            }
            guard let review = review(for: draft) else {
                errorMessage = "The Review for this draft is unavailable. It may have already been closed."
                return
            }
            openReview(review)
        } catch {
            guard workspaceReloadGeneration == generation,
                  activeProjectId == projectId,
                  selectedSection == section, !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }

    var submittedProjectDrafts: [LocalDraft] {
        guard let activeProjectId else { return [] }
        return drafts.filter { $0.projectId == activeProjectId && $0.status == .submitted }
    }

    func canPerformReviewMenuAction(_ action: ReviewMenuAction) -> Bool {
        guard phase == .ready,
              selectedSection == .reviews,
              let selectedReviewId,
              let review = reviews.first(where: { $0.id == selectedReviewId }),
              reviewDecisionReadiness?.matches(review) == true,
              review.freshness == .current else {
            return false
        }
        return action.isAvailable(
            for: review,
            canDecideReviews: canDecideReviews,
            canMergeReviews: canMergeReviews,
            isAuthor: isReviewAuthor(review)
        )
    }

    func performReviewMenuAction(_ action: ReviewMenuAction) async {
        guard canPerformReviewMenuAction(action),
              let selectedReviewId,
              let review = reviews.first(where: { $0.id == selectedReviewId }),
              let readiness = reviewDecisionReadiness else { return }
        reviewDecisionReadiness = nil
        do {
            switch action {
            case .approve:
                try await merge(review)
            case .reject:
                try await decide(review, decision: "rejected", note: "")
            case .merge:
                try await merge(review)
            case .resubmit:
                let detail = try await reviewDetail(review.id)
                if detail.draft.coordination.freshness == .behind {
                    pendingReviewReconciliationId = review.id
                } else {
                    try await resubmit(review, detail: detail)
                }
            }
        } catch {
            if self.selectedReviewId == selectedReviewId,
               selectedSection == .reviews,
               let currentReview = reviews.first(where: { $0.id == selectedReviewId }),
               readiness.matches(currentReview) {
                reviewDecisionReadiness = readiness
            }
            errorMessage = error.localizedDescription
        }
    }

    var visibleMemoryItems: [MemoryListItem] {
        switch selectedSection {
        case .memory:
            break
        case .bundles, .reviews, .sessions:
            return []
        }

        let authoritative = Self.memoryTreeResources(
            resources,
            activeProjectId: activeProjectId,
            selectedOrgResourceIds: activeProject?.selectedOrgResourceIds ?? []
        )
        let activeDrafts = Self.preferredMemoryTreeDrafts(
            Self.memoryTreeDrafts(drafts, activeProjectId: activeProjectId)
        )
        let draftByTarget = Dictionary(
            activeDrafts.compactMap { draft in draft.targetId.map { ($0, draft) } },
            uniquingKeysWith: { current, candidate in
                candidate.updatedAt > current.updatedAt ? candidate : current
            }
        )
        var items = authoritative.map { resource in
            MemoryListItem(
                id: resource.id,
                resource: resource,
                draft: draftByTarget[resource.id],
                inherited: resource.scope == .org
                    && activeProjectId != nil
                    && (activeProject?.selectedOrgResourceIds.contains(resource.id) ?? false),
                projectContextId: activeProjectId
            )
        }
        let authoritativeIds = Set(authoritative.map(\.id))
        items.append(contentsOf: Self.unrepresentedDrafts(
            activeDrafts,
            authoritativeResourceIds: authoritativeIds
        ).map {
            MemoryListItem(
                id: $0.targetId ?? $0.id,
                resource: nil,
                draft: $0,
                inherited: false,
                projectContextId: activeProjectId
            )
        })
        return items.sorted { $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending }
    }

    nonisolated static func filterMemoryItems(
        _ items: [MemoryListItem],
        query: String
    ) -> [MemoryListItem] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        guard !needle.isEmpty else { return items }
        return items.filter { item in
            let document = item.document
            return "\(document.title) \(document.path) \(document.body) \(item.kind.title)"
                .localizedLowercase.contains(needle)
        }
    }

    nonisolated static func filterBundles(
        _ bundles: [PersonalBundle],
        query: String
    ) -> [PersonalBundle] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        guard !needle.isEmpty else { return bundles }
        return bundles.filter {
            "\($0.name) \($0.description)".localizedLowercase.contains(needle)
        }
    }

    /// The Project tree is the Project's effective Memory surface: selected
    /// Org authority plus its local Draft overlay. Existing project-scope
    /// authority remains visible as a compatibility layer until that legacy
    /// publication path is migrated separately.
    nonisolated static func memoryTreeResources(
        _ resources: [MemoryResource],
        activeProjectId: String?,
        selectedOrgResourceIds: Set<String>
    ) -> [MemoryResource] {
        guard let activeProjectId else {
            return resources.filter { $0.scope == .org }
        }
        return resources.filter { resource in
            switch resource.scope {
            case .org:
                selectedOrgResourceIds.contains(resource.id)
            case .project:
                resource.projectId == activeProjectId
            }
        }
    }

    nonisolated static func memoryTreeDrafts(
        _ drafts: [LocalDraft],
        activeProjectId: String?
    ) -> [LocalDraft] {
        // LocalDraft is always a Project-bound overlay, including an Org-
        // targeted Draft. The Org catalog presents shared authority only.
        guard let activeProjectId else { return [] }
        return drafts.filter { draft in
            guard draft.status != .discarded && draft.status != .merged else {
                return false
            }
            return draft.projectId == activeProjectId
        }
    }

    nonisolated static func preferredMemoryTreeDrafts(
        _ drafts: [LocalDraft]
    ) -> [LocalDraft] {
        var preferred: [String: LocalDraft] = [:]
        for draft in drafts {
            let key = draft.targetId ?? "draft:\(draft.id)"
            if let current = preferred[key], current.updatedAt >= draft.updatedAt {
                continue
            }
            preferred[key] = draft
        }
        return preferred.values.sorted { lhs, rhs in
            if lhs.updatedAt != rhs.updatedAt { return lhs.updatedAt > rhs.updatedAt }
            return lhs.id < rhs.id
        }
    }

    nonisolated static func hasActiveDraft(
        in projectId: String,
        targetingAny resourceIds: Set<String>,
        drafts: [LocalDraft]
    ) -> Bool {
        drafts.contains { draft in
            draft.projectId == projectId
                && draft.status != .discarded
                && draft.status != .merged
                && draft.targetId.map(resourceIds.contains) == true
        }
    }

    nonisolated static func memoryTabDraft(
        itemId: String,
        projectId: String?,
        drafts: [LocalDraft]
    ) -> LocalDraft? {
        guard let projectId else { return nil }
        let matchingDrafts = drafts.filter { draft in
            (draft.id == itemId || draft.targetId == itemId)
                && draft.projectId == projectId
                && draft.status != .discarded
                && draft.status != .merged
        }
        return preferredMemoryTreeDrafts(matchingDrafts).first
    }

    nonisolated static func unrepresentedDrafts(
        _ activeDrafts: [LocalDraft],
        authoritativeResourceIds: Set<String>
    ) -> [LocalDraft] {
        activeDrafts.filter { draft in
            guard let targetId = draft.targetId else { return true }
            return !authoritativeResourceIds.contains(targetId)
        }
    }

    nonisolated static func resourceGenerationMatches(
        _ lhs: MemoryResource,
        _ rhs: MemoryResource
    ) -> Bool {
        lhs.id == rhs.id
            && lhs.scope == rhs.scope
            && lhs.projectId == rhs.projectId
            && lhs.kind == rhs.kind
            && lhs.contentHash == rhs.contentHash
            && lhs.refCommitId == rhs.refCommitId
            && lhs.document.path == rhs.document.path
    }

    nonisolated static func preservesDeferredAuthority(
        currentAccount: UserReference?,
        currentOrganization: OrganizationReference?,
        nextAccount: UserReference,
        nextOrganization: OrganizationReference
    ) -> Bool {
        currentAccount?.userId == nextAccount.userId
            && currentOrganization?.orgId == nextOrganization.orgId
    }

    nonisolated static func invalidateWorkspaceTransitionState(
        generation: inout UUID,
        loadingProjectId: inout String?,
        isSwitchingMemoryContext: inout Bool,
        isPreparingWorkspaceIndex: inout Bool,
        orgResourceRefreshGeneration: inout UUID?
    ) {
        generation = UUID()
        loadingProjectId = nil
        isSwitchingMemoryContext = false
        isPreparingWorkspaceIndex = false
        orgResourceRefreshGeneration = nil
    }

    nonisolated static func rejectsStaleAuthorityChange(
        hadLoadedWorkspace: Bool,
        sameAuthority: Bool,
        snapshotWasStale: Bool
    ) -> Bool {
        hadLoadedWorkspace && !sameAuthority && snapshotWasStale
    }

    nonisolated static func deferredLoadRequiresFreshData(
        hadLoadedWorkspace: Bool
    ) -> Bool {
        hadLoadedWorkspace
    }

    nonisolated static func retainingAccessibleProjectRecords<Record>(
        _ records: [Record],
        accessibleProjectIds: Set<String>,
        projectId: KeyPath<Record, String>
    ) -> [Record] {
        records.filter { accessibleProjectIds.contains($0[keyPath: projectId]) }
    }

    nonisolated static func canPublishDeferredLoad(
        requiresFreshData: Bool,
        baseSnapshotWasStale: Bool,
        responseWasStale: Bool
    ) -> Bool {
        !requiresFreshData || (!baseSnapshotWasStale && !responseWasStale)
    }

    nonisolated static func mergeDeferredRecords<Record>(
        baseline: [Record],
        current: [Record],
        loaded: [Record]
    ) -> [Record] where Record: Identifiable & Equatable, Record.ID: Hashable {
        let baselineById = Dictionary(
            baseline.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        let currentById = Dictionary(
            current.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        let loadedIds = Set(loaded.map(\.id))
        var merged = current.filter {
            !loadedIds.contains($0.id) && currentById[$0.id] != baselineById[$0.id]
        }
        merged.append(contentsOf: loaded.compactMap { loadedRecord -> Record? in
            let id = loadedRecord.id
            guard currentById[id] != baselineById[id] else { return loadedRecord }
            return currentById[id]
        })
        return merged
    }

    @discardableResult
    private func installLoadedResourceIfCurrent(_ loaded: MemoryResource) -> Bool {
        guard let index = resources.firstIndex(where: { $0.id == loaded.id }),
              !resources[index].contentLoaded,
              Self.resourceGenerationMatches(resources[index], loaded) else {
            return false
        }
        resources[index] = loaded
        bumpDocumentContentGeneration(for: loaded.id)
        return true
    }

    func start() {
        Task { await reload() }
    }

    func reload(allowsDuringDocumentReconciliation: Bool = false) async {
        guard !isSigningOut else { return }
        guard allowsDuringDocumentReconciliation
                || (applyingDocumentReconciliationSessions.isEmpty
                    && standaloneReconciliationActivityIds.isEmpty) else {
            return
        }
        guard loadingProjectId == nil, !isSwitchingMemoryContext else { return }
        if phase == .ready, hasPendingChanges, !(await flushPendingChanges()) {
            return
        }
        // A project selection may have started while pending edits were
        // flushing. Let that serialized intent finish before a later reload.
        guard loadingProjectId == nil, !isSwitchingMemoryContext else { return }
        cancelPostReadyWork()
        resetBackgroundErrorPresentation()
        let generation = UUID()
        workspaceReloadGeneration = generation
        let hadLoadedWorkspace = account != nil
        phase = .loading
        errorMessage = nil
        do {
            let snapshot = try await WorkspaceLoader(
                daemon: daemon,
                bootstrap: bootstrap,
                server: server
            ).load { [weak self] result in
                guard self?.workspaceReloadGeneration == generation else { return }
                self?.applyLocalAgentAdapterResult(result)
            }
            guard workspaceReloadGeneration == generation else { return }
            let sameAuthority = Self.preservesDeferredAuthority(
                currentAccount: account,
                currentOrganization: organization,
                nextAccount: snapshot.account,
                nextOrganization: snapshot.organization
            )
            let snapshotWasStale = snapshot.runtime.serverDataSource == "stale"
            if Self.rejectsStaleAuthorityChange(
                hadLoadedWorkspace: hadLoadedWorkspace,
                sameAuthority: sameAuthority,
                snapshotWasStale: snapshotWasStale
            ) {
                clearAuthorityScopedWorkspace()
                phase = .failed(
                    "Fresh account data is required before switching workspaces. "
                        + "The previous account workspace was cleared."
                )
                return
            }
            let preservesLoadedWorkspace = hadLoadedWorkspace && sameAuthority
            // Stale-cache data keeps a cold start usable while offline, but it
            // must never replace a newer generation already held in memory.
            guard !preservesLoadedWorkspace || !snapshotWasStale else {
                phase = .ready
                startPostReadyWork(
                    generation: generation,
                    requiresFreshData: true,
                    baseSnapshotWasStale: true
                )
                return
            }
            apply(snapshot)
            phase = .ready
            startPostReadyWork(
                generation: generation,
                requiresFreshData: Self.deferredLoadRequiresFreshData(
                    hadLoadedWorkspace: hadLoadedWorkspace
                ),
                baseSnapshotWasStale: false
            )
        } catch WorkspaceLoadError.authenticationRequired {
            guard workspaceReloadGeneration == generation else { return }
            phase = .authenticationRequired
        } catch {
            guard workspaceReloadGeneration == generation else { return }
            let messages = [error.localizedDescription, errorMessage]
                .compactMap { $0 }
                .filter { !$0.isEmpty }
            phase = .failed(messages.joined(separator: "\n\n"))
        }
    }

    func presentProjectCreation() {
        guard canCreateProject else { return }
        showsProjectCreation = true
    }

    func createProject(
        name: String,
        description: String,
        idempotencyKey: String,
        repositoryPaths: [String],
        bundleId: String?
    ) async throws -> String {
        guard canCreateProject else { throw AdministrationError.forbidden }
        guard phase == .ready else { throw AdministrationError.unavailable }
        let generation = try beginAdministrationMutation()
        defer { finishAdministrationMutation(generation) }
        let repositoryPaths = normalizedRepositoryPaths(repositoryPaths)
        let created: ProjectRecord = try await server.send(
            method: "POST",
            path: "/api/v1/projects",
            headers: ["idempotency-key": idempotencyKey],
            body: CreateProjectRequest(
                name: name,
                description: description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    ? nil
                    : description
            )
        )
        try ensureCurrentAdministrationMutation(generation)
        projectRoles[created.id] = .admin
        let initialSelection = try await initializeProjectMemory(
            projectId: created.id,
            bundleId: bundleId,
            generation: generation
        )
        try ensureCurrentAdministrationMutation(generation)
        let project = ProjectState(
            id: created.id,
            name: created.name,
            refCommitId: nil,
            refEtag: "",
            selectedOrgResourceIds: projectOrgResourceIds(initialSelection),
            orgSelectionRevision: initialSelection.revision,
            isLoaded: false
        )
        if let index = projects.firstIndex(where: { $0.id == created.id }) {
            projects[index] = project
        } else {
            projects.insert(project, at: 0)
        }
        for repositoryPath in repositoryPaths {
            _ = try await daemon.replaceProjectBinding(
                .init(
                    workspaceRoot: repositoryPath,
                    projectId: created.id,
                    expectedRevision: nil
                )
            )
            try ensureCurrentAdministrationMutation(generation)
            projectBindingsGeneration = UUID()
        }
        projectDirectoryChanges.send()
        return created.id
    }

    func beginAdministrationMutation() throws -> UUID {
        guard phase == .ready else { throw AdministrationError.unavailable }
        guard !isMutatingAdministration else { throw AdministrationError.busy }
        let generation = UUID()
        administrationMutationGeneration = generation
        isMutatingAdministration = true
        return generation
    }

    func ensureCurrentAdministrationMutation(_ generation: UUID) throws {
        guard administrationMutationGeneration == generation,
              phase == .ready else {
            throw CancellationError()
        }
    }

    func finishAdministrationMutation(_ generation: UUID) {
        guard administrationMutationGeneration == generation else { return }
        isMutatingAdministration = false
    }

    func updateOrganization(_ organization: OrganizationReference) {
        self.organization = organization
    }

    func removeProjectRole(_ projectId: String) {
        projectRoles[projectId] = nil
    }

    func projectBindings(_ projectId: String) async throws -> [DaemonProjectBinding] {
        try await daemon.projectBindings(projectId)
    }

    func addProjectRepositories(
        _ repositoryPaths: [String],
        projectId: String
    ) async throws -> [DaemonProjectBinding] {
        var bindings: [DaemonProjectBinding] = []
        for repositoryPath in normalizedRepositoryPaths(repositoryPaths) {
            bindings.append(try await daemon.replaceProjectBinding(
                .init(
                    workspaceRoot: repositoryPath,
                    projectId: projectId,
                    expectedRevision: nil
                )
            ))
            projectBindingsGeneration = UUID()
        }
        return bindings
    }

    func removeProjectRepository(_ binding: DaemonProjectBinding) async throws {
        var didMutate = false
        defer {
            if didMutate {
                projectBindingsGeneration = UUID()
            }
        }
        let adapters = try await projectAgentAdapters(binding.projectId)
        for adapter in adapters where adapter.workspaceRoot == binding.workspaceRoot {
            _ = try await daemon.removeProjectAgentAdapter(
                .init(
                    workspaceRoot: binding.workspaceRoot,
                    adapter: adapter.adapter,
                    expectedRevision: adapter.revision
                )
            )
            didMutate = true
        }
        _ = try await daemon.removeProjectBinding(
            .init(
                workspaceRoot: binding.workspaceRoot,
                expectedRevision: binding.revision
            )
        )
        didMutate = true
    }

    func projectAgentAdapters(_ projectId: String) async throws -> [DaemonProjectAgentAdapter] {
        try await daemon.projectAgentAdapters(projectId)
    }

    func allProjectAgentAdapters() async throws -> [DaemonProjectAgentAdapter] {
        try await daemon.allProjectAgentAdapters()
    }

    func codexPluginStatus() async throws -> DaemonCodexPluginStatus {
        try await daemon.inspectCodexPlugin(
            .init(
                runtimeBinaryPath: try bundledAgentRuntimePath(),
                hostBinaryPath: try? WorkspaceLoader.installedCodexHostBinaryPath()
            )
        )
    }

    func setAgentAdapter(_ adapter: ProjectAgentAdapterKind, enabled: Bool) async throws -> [DaemonAgentAdapterSetting] {
        try await daemon.setAgentAdapter(.init(
            adapter: adapter,
            enabled: enabled,
            runtimeBinaryPath: try bundledAgentRuntimePath(),
            hostBinaryPath: adapter == .codex ? try? WorkspaceLoader.installedCodexHostBinaryPath() : nil
        ))
    }

    func showOrgMemory() async {
        guard phase == .ready else { return }
        guard activeProjectId != nil || loadingProjectId != nil else { return }
        guard canCommitMemoryContextSwitch else {
            errorMessage = "Finish or cancel the active document Sync before switching Memory context."
            return
        }
        // Showing Org supersedes every in-flight Project selection. A stale
        // daemon reply must not be allowed to switch the UI back afterward.
        let generation = UUID()
        projectSelectionGeneration = generation
        loadingProjectId = nil
        isSwitchingMemoryContext = true
        clearPendingDocumentSessionPresentation()
        defer {
            if projectSelectionGeneration == generation {
                isSwitchingMemoryContext = false
            }
        }
        guard await flushPendingDocumentChanges(),
              projectSelectionGeneration == generation else { return }
        guard canCommitMemoryContextSwitch else {
            errorMessage = "Finish or cancel the active document Sync before switching Memory context."
            return
        }
        clearPendingDocumentSessionPresentation()
        activeProjectId = nil
        refreshVisibleStaleResourceIds()
        showsProjectSettings = false
        selectedItemId = nil
        let tab = visibleTabs.last
        activeTabId = tab?.id
    }

    func selectProject(_ projectId: String) async {
        guard phase == .ready else { return }
        guard let project = projects.first(where: { $0.id == projectId }) else { return }
        if projectId == activeProjectId,
           project.isLoaded,
           loadingProjectId == nil,
           !isSwitchingMemoryContext {
            return
        }
        guard canCommitMemoryContextSwitch else {
            errorMessage = "Finish or cancel the active document Sync before switching Memory context."
            return
        }
        let generation = UUID()
        let workspaceGeneration = workspaceReloadGeneration
        // Publish the newest intent before the first suspension point. This
        // closes the window where two clicks could both start a daemon-side
        // selection while a pending document save was flushing.
        projectSelectionGeneration = generation
        loadingProjectId = projectId
        isSwitchingMemoryContext = true
        clearPendingDocumentSessionPresentation()
        defer {
            if projectSelectionGeneration == generation {
                loadingProjectId = nil
                isSwitchingMemoryContext = false
            }
        }
        guard await flushPendingDocumentChanges(),
              projectSelectionGeneration == generation else { return }
        guard canCommitMemoryContextSwitch else {
            errorMessage = "Finish or cancel the active document Sync before switching Memory context."
            return
        }
        do {
            let selectedLatestIntent = try await projectSelectionSideEffectGate.run {
                guard projectSelectionGeneration == generation else { return false }
                _ = try await daemon.selectProject(projectId)
                return true
            }
            guard selectedLatestIntent,
                  projectSelectionGeneration == generation else { return }
            guard canCommitMemoryContextSwitch else {
                errorMessage = "Finish or cancel the active document Sync before switching Memory context."
                return
            }
            clearPendingDocumentSessionPresentation()
            activeProjectId = projectId
            refreshVisibleStaleResourceIds()
            let tab = visibleTabs.last
            activeTabId = tab?.id
            selectedItemId = tab?.itemId

            let loader = WorkspaceLoader(daemon: daemon, bootstrap: bootstrap, server: server)
            var loadedProject: (state: ProjectState, resources: [MemoryResource])?
            var needsBackgroundRefresh = project.isLoaded
            if !project.isLoaded {
                if let cached = try await loader.loadCachedProject(id: project.id, name: project.name) {
                    loadedProject = cached
                    needsBackgroundRefresh = true
                } else {
                    loadedProject = try await loader.loadProject(id: project.id, name: project.name)
                }
            }
            guard projectSelectionGeneration == generation else { return }
            if let loadedProject {
                if let index = projects.firstIndex(where: { $0.id == projectId }) {
                    projects[index] = loadedProject.state
                }
                replaceProjectResources(projectId: projectId, with: loadedProject.resources)
                try await remapDrafts(
                    projectId: projectId,
                    workspaceGeneration: workspaceGeneration
                )
            }
            guard projectSelectionGeneration == generation else { return }
            if needsBackgroundRefresh {
                Task { [weak self] in
                    await self?.refreshProjectFromServer(
                        projectId: project.id,
                        projectName: project.name,
                        generation: generation
                    )
                }
            }
        } catch {
            guard projectSelectionGeneration == generation else { return }
            errorMessage = error.localizedDescription
        }
    }

    func focusWorkspaceSearch() {
        workspaceSearchFocusToken = UUID()
    }

    func focusReviewSearch() {
        reviewSearchFocusToken = UUID()
    }

    func prepareWorkspaceIndex(includeContent: Bool) async {
        let generation = workspaceReloadGeneration
        while isPreparingWorkspaceIndex {
            try? await Task.sleep(for: .milliseconds(100))
            guard workspaceReloadGeneration == generation else { return }
        }
        guard workspaceReloadGeneration == generation else { return }
        let needsProjects = projects.contains { !$0.isLoaded }
        let needsContent = includeContent && resources.contains { !$0.contentLoaded }
        guard needsProjects || needsContent else { return }

        isPreparingWorkspaceIndex = true
        defer {
            if workspaceReloadGeneration == generation {
                isPreparingWorkspaceIndex = false
            }
        }
        do {
            let loader = WorkspaceLoader(daemon: daemon, bootstrap: bootstrap, server: server)
            let unloadedProjects = projects.filter { !$0.isLoaded }
            let loadedProjects = try await concurrentMap(unloadedProjects, maxConcurrent: 4) { project in
                try await loader.loadProject(id: project.id, name: project.name)
            }
            guard workspaceReloadGeneration == generation else { return }
            for loaded in loadedProjects {
                clearStaleResourceState(for: loaded.state.id)
                if let index = projects.firstIndex(where: { $0.id == loaded.state.id }) {
                    projects[index] = loaded.state
                }
                replaceProjectResources(projectId: loaded.state.id, with: loaded.resources)
            }

            if includeContent {
                let unloadedResources = resources.filter { !$0.contentLoaded }
                let loadedResources = try await concurrentMap(unloadedResources) {
                    try await loader.loadContent(for: $0)
                }
                guard workspaceReloadGeneration == generation else { return }
                for loaded in loadedResources {
                    installLoadedResourceIfCurrent(loaded)
                }
            }

            for projectId in Set(drafts.map(\.projectId)) {
                try await remapDrafts(
                    projectId: projectId,
                    workspaceGeneration: generation
                )
                guard workspaceReloadGeneration == generation else { return }
            }
        } catch is CancellationError {
            return
        } catch {
            guard workspaceReloadGeneration == generation else { return }
            errorMessage = error.localizedDescription
        }
    }

    func open(_ item: MemoryListItem, mode: WorkbenchTabMode? = nil) {
        guard let item = Self.memoryItemForViewContext(
            item,
            activeProjectId: activeProjectId
        ) else { return }
        showsProjectSettings = false
        let previousTabId = activeVisibleTab?.id
        // Project-local overlays need a separate session from the Org
        // authority view even when both address the same Org memory id.
        let tabProjectId = activeProjectId ?? (item.scope == .org ? nil : item.projectId)
        let existingMode = tabs.first {
            $0.section == selectedSection
                && $0.projectId == tabProjectId
                && $0.itemId == item.id
        }?.mode
        let resolvedMode = mode
            ?? existingMode
            ?? (item.supportsMarkdownPreview ? .preview : .source)
        let compatibleMode: WorkbenchTabMode = resolvedMode == .preview
            && !item.supportsMarkdownPreview ? .source : resolvedMode
        let safeMode: WorkbenchTabMode = item.draft?.documentBaselineAvailable == false
            ? .diff : compatibleMode
        let requestedTab = WorkbenchTab(
            section: selectedSection,
            projectId: tabProjectId,
            itemId: item.id,
            mode: safeMode,
            title: item.document.title
        )
        let tab = installDocumentTab(requestedTab)
        selectedItemId = item.id
        if let previousTabId, previousTabId != tab.id {
            navigationBackStack.append(previousTabId)
            navigationForwardStack.removeAll()
        }
        activeTabId = tab.id
        Task { await loadContentIfNeeded(item) }
    }

    nonisolated static func memoryItemForViewContext(
        _ item: MemoryListItem,
        activeProjectId: String?
    ) -> MemoryListItem? {
        guard let activeProjectId else {
            guard let resource = item.resource, resource.scope == .org else { return nil }
            return MemoryListItem(
                id: resource.id,
                resource: resource,
                draft: nil,
                inherited: false,
                projectContextId: nil
            )
        }
        let itemProjectId = item.projectContextId
            ?? item.draft?.projectId
            ?? (item.scope == .project ? item.projectId : nil)
        guard itemProjectId == nil || itemProjectId == activeProjectId else { return nil }
        return item
    }

    /// Switches the active tab's view mode in place instead of stacking a
    /// second tab for the same document.
    func switchDocumentMode(_ mode: WorkbenchTabMode) {
        guard let tab = activeVisibleTab, tab.mode != mode else { return }
        var updated = tab
        updated.mode = mode
        updated = installDocumentTab(updated)
        selectedItemId = tab.itemId
        activeTabId = updated.id
    }

    /// Installs one stable tab per document. Older builds encoded the mode in
    /// the tab identity, so this also collapses any duplicate mode tabs that
    /// survived in memory while the view hierarchy was updating.
    @discardableResult
    private func installDocumentTab(_ requested: WorkbenchTab) -> WorkbenchTab {
        let matches = tabs.indices.filter { index in
            let candidate = tabs[index]
            return candidate.section == requested.section
                && candidate.projectId == requested.projectId
                && candidate.itemId == requested.itemId
        }
        guard let first = matches.first else {
            tabs.append(requested)
            return requested
        }
        tabs[first] = requested
        for index in matches.dropFirst().reversed() {
            tabs.remove(at: index)
        }
        return requested
    }

    private func refreshDocumentTabs(for itemId: String) {
        for index in tabs.indices where tabs[index].itemId == itemId {
            guard let item = item(for: tabs[index]) else { continue }
            tabs[index].title = item.document.title
            if item.draft?.documentBaselineAvailable == false {
                tabs[index].mode = .diff
            } else if tabs[index].mode == .preview, !item.supportsMarkdownPreview {
                tabs[index].mode = .source
            }
        }
    }

    private func refreshAllDocumentTabs() {
        for itemId in Set(tabs.map(\.itemId)) {
            refreshDocumentTabs(for: itemId)
        }
    }

    /// Remove document sessions that no longer exist in their own view
    /// context. In particular, a clean selected Org memory stops belonging to
    /// Project P as soon as P removes it, even though the Org authority remains
    /// globally live. A P-bound LocalDraft still retains P's draft-only tab.
    private func pruneOrphanedMemoryTabs() {
        let retainedTabs = Self.retainedMemoryTabs(
            tabs,
            projects: projects,
            resources: resources,
            drafts: drafts
        )
        let retainedTabIds = Set(retainedTabs.map(\.id))
        let removedTabs = tabs.filter { !retainedTabIds.contains($0.id) }
        let removedTabIds = Set(removedTabs.map(\.id))
        guard !removedTabIds.isEmpty else { return }
        for tab in removedTabs {
            clearDocumentSynchronizationState(for: tab)
        }
        tabs = retainedTabs
        navigationBackStack.removeAll { removedTabIds.contains($0) }
        navigationForwardStack.removeAll { removedTabIds.contains($0) }
        if let activeTabId, removedTabIds.contains(activeTabId) {
            if let activeTab = removedTabs.first(where: { $0.id == activeTabId }) {
                if let sessionKey = documentSessionKey(for: activeTab) {
                    if pendingDocumentCommand?.sessionKey == sessionKey {
                        pendingDocumentCommand = nil
                    }
                    if documentReconciliationToolbarState?.sessionKey == sessionKey {
                        documentReconciliationToolbarState = nil
                    }
                }
            }
            self.activeTabId = nil
            selectedItemId = nil
        }
    }

    func reveal(_ item: MemoryListItem) async {
        selectedSection = .memory
        selectedKind = item.kind
        if let projectId = item.projectContextId {
            await selectProject(projectId)
            guard activeProjectId == projectId else { return }
        } else if item.scope == .project, let projectId = item.projectId {
            await selectProject(projectId)
            guard activeProjectId == projectId else { return }
        } else if activeProjectId != nil,
                  !(activeProject?.selectedOrgResourceIds.contains(item.id) ?? false) {
            await showOrgMemory()
            guard activeProjectId == nil else { return }
        }
        open(item)
    }

    func item(for tab: WorkbenchTab) -> MemoryListItem? {
        if let resource = resources.first(where: { $0.id == tab.itemId }) {
            guard tab.projectId != nil || resource.scope == .org else { return nil }
            let draft = Self.memoryTabDraft(
                itemId: resource.id,
                projectId: tab.projectId,
                drafts: drafts
            )
            let tabProject = tab.projectId.flatMap { projectId in
                projects.first { $0.id == projectId }
            }
            return .init(
                id: resource.id,
                resource: resource,
                draft: draft,
                inherited: resource.scope == .org
                    && tabProject != nil
                    && (tabProject?.selectedOrgResourceIds.contains(resource.id) ?? false),
                projectContextId: tab.projectId
            )
        }
        if let draft = Self.memoryTabDraft(
            itemId: tab.itemId,
            projectId: tab.projectId,
            drafts: drafts
        ) {
            let resource = draft.targetId.flatMap { target in resources.first { $0.id == target } }
            return .init(
                id: resource?.id ?? draft.targetId ?? draft.id,
                resource: resource,
                draft: draft,
                inherited: false,
                projectContextId: tab.projectId
            )
        }
        return nil
    }

    func canExportMemory(_ items: [MemoryListItem]) -> Bool {
        phase == .ready && !isExportingMemory && !isSwitchingMemoryContext
            && (activeProjectId == nil || activeProject?.isLoaded == true)
            && (activeProjectId == nil || draftInventoryLoadState == .loaded)
            && items.contains { $0.draft?.isDeletion != true }
            && !items.contains { isSynchronizingDocument($0.id) }
    }

    func exportMemory(_ selection: [MemoryListItem]? = nil, name: String? = nil) {
        let items = selection ?? visibleMemoryItems
        guard canExportMemory(items) else { return }
        // Capture the view and pending editor text before the save panel can change context.
        let pendingDocuments = Dictionary(items.compactMap { item in
            pendingDocument(for: item).map { (item.id, $0) }
        }, uniquingKeysWith: { _, latest in latest })
        let loader = WorkspaceLoader(daemon: daemon, bootstrap: bootstrap, server: server)
        let panel = NSSavePanel()
        panel.title = "Export Memory"
        panel.prompt = "Export"
        panel.message = "Export current files, including local draft edits. Deleted memories are excluded."
        let baseName = name ?? (selection == nil ? activeProject?.name : nil) ?? "Memory"
        panel.nameFieldStringValue = baseName.replacingOccurrences(of: "/", with: "-")
            .replacingOccurrences(of: ":", with: "-") + ".zip"
        panel.allowedContentTypes = [.zip]
        panel.canCreateDirectories = true
        isExportingMemory = true
        panel.begin { result in
            Task { @MainActor in
                defer { self.isExportingMemory = false }
                guard result == .OK, let destination = panel.url else { return }
                do {
                    let documents = try await Self.memoryExportDocuments(
                        items, pendingDocuments: pendingDocuments
                    ) { try await loader.loadContent(for: $0) }
                    try await Task.detached(priority: .userInitiated) {
                        try MemoryArchive.write(documents, to: destination)
                    }.value
                    NSWorkspace.shared.activateFileViewerSelecting([destination])
                } catch {
                    self.errorMessage = "Could Not Export Memory: \(error.localizedDescription)"
                }
            }
        }
    }

    nonisolated static func memoryExportDocuments(
        _ items: [MemoryListItem],
        pendingDocuments: [String: EditableMemoryDocument] = [:],
        loadContent: @escaping @Sendable (MemoryResource) async throws -> MemoryResource
    ) async throws -> [EditableMemoryDocument] {
        let files = items.filter { $0.draft?.isDeletion != true }
        guard !files.isEmpty else { throw MemoryExportError.empty }
        return try await concurrentMap(files) { item in
            if let pending = pendingDocuments[item.id] { return pending }
            if item.contentLoaded { return item.document }
            guard item.draft == nil, let resource = item.resource else {
                throw MemoryExportError.contentUnavailable(item.document.path)
            }
            let loaded = try await loadContent(resource)
            guard loaded.contentLoaded else {
                throw MemoryExportError.contentUnavailable(item.document.path)
            }
            return loaded.document
        }
    }

    @discardableResult
    func loadContentIfNeeded(
        _ item: MemoryListItem,
        loadContent: (@Sendable (MemoryResource) async throws -> MemoryResource)? = nil
    ) async -> String? {
        guard !Task.isCancelled else { return nil }
        guard item.draft == nil,
              let resource = item.resource,
              !resource.contentLoaded else { return nil }
        if let inFlight = resourceLoadRequests[resource.id],
           Self.resourceGenerationMatches(inFlight.resource, resource) {
            return await inFlight.task.value
        }
        resourceLoadRequests[resource.id]?.task.cancel()
        if let snapshot = staleResourceSnapshot(for: item) {
            guard let local = snapshot.local, local.contentLoaded else {
                return DocumentDiffError.baselineUnavailable.localizedDescription
            }
            if let index = resources.firstIndex(where: { $0.id == resource.id }) {
                resources[index] = local
                bumpDocumentContentGeneration(for: resource.id)
            }
            return nil
        }
        let generation = UUID()
        let task = Task { @MainActor [weak self] in
            await self?.loadResourceContent(resource, generation: generation, loadContent: loadContent)
        }
        resourceLoadRequests[resource.id] = .init(resource: resource, generation: generation, task: task)
        loadingResourceIds.insert(resource.id)
        return await task.value
    }

    private func loadResourceContent(
        _ resource: MemoryResource,
        generation: UUID,
        loadContent: (@Sendable (MemoryResource) async throws -> MemoryResource)?
    ) async -> String? {
        defer {
            if resourceLoadRequests[resource.id]?.generation == generation {
                resourceLoadRequests.removeValue(forKey: resource.id)
                loadingResourceIds.remove(resource.id)
            }
        }
        do {
            let loaded: MemoryResource
            if let loadContent {
                loaded = try await loadContent(resource)
            } else {
                loaded = try await WorkspaceLoader(
                    daemon: daemon, bootstrap: bootstrap, server: server
                ).loadContent(for: resource)
            }
            try Task.checkCancellation()
            guard resourceLoadRequests[resource.id]?.generation == generation else { return nil }
            installLoadedResourceIfCurrent(loaded)
        } catch is CancellationError {
            return nil
        } catch {
            guard !Task.isCancelled,
                  resourceLoadRequests[resource.id]?.generation == generation else { return nil }
            return error.localizedDescription
        }
        return nil
    }

    func closeTab(_ tab: WorkbenchTab) {
        if let key = documentSessionKey(for: tab),
           applyingDocumentReconciliationSessions.contains(key) {
            errorMessage = "Wait for the shared update to finish before closing this tab."
            return
        }
        guard let index = tabs.firstIndex(where: { $0.id == tab.id }) else { return }
        if let sessionKey = documentSessionKey(for: tab) {
            if pendingDocumentCommand?.sessionKey == sessionKey {
                pendingDocumentCommand = nil
            }
            if documentReconciliationToolbarState?.sessionKey == sessionKey {
                documentReconciliationToolbarState = nil
            }
        }
        clearDocumentSynchronizationState(for: tab)
        let visibleIndex = visibleTabs.firstIndex(where: { $0.id == tab.id })
        tabs.remove(at: index)
        navigationBackStack.removeAll { $0 == tab.id }
        navigationForwardStack.removeAll { $0 == tab.id }
        if activeTabId == tab.id {
            let remaining = visibleTabs
            let replacementIndex = min(visibleIndex ?? remaining.count, remaining.count - 1)
            activeTabId = replacementIndex >= 0 ? remaining[replacementIndex].id : nil
            selectedItemId = tabs.first { $0.id == activeTabId }?.itemId
        }
    }

    private func clearDocumentSynchronizationState(for tab: WorkbenchTab) {
        guard let key = documentSessionKey(for: tab) else { return }
        documentSynchronizationTasks.removeValue(forKey: key)?.cancel()
        pendingDocumentReconciliationCandidatesBySession.removeValue(forKey: key)
        documentReconciliationResolutions.removeValue(forKey: key)
        synchronizingDocumentSessions.remove(key)
        documentSynchronizationGenerations.removeValue(forKey: key)
    }

    func selectTab(_ tab: WorkbenchTab) {
        let previousTabId = activeVisibleTab?.id
        guard tab.id != previousTabId else {
            activeTabId = tab.id
            selectedItemId = tab.itemId
            return
        }
        if let previousTabId {
            navigationBackStack.append(previousTabId)
            navigationForwardStack.removeAll()
        }
        activeTabId = tab.id
        selectedItemId = tab.itemId
    }

    func goBack() {
        while let previousId = navigationBackStack.popLast() {
            guard isVisibleTab(previousId) else { continue }
            if let currentId = activeVisibleTab?.id {
                navigationForwardStack.append(currentId)
            }
            activateTab(previousId)
            return
        }
    }

    func goForward() {
        while let nextId = navigationForwardStack.popLast() {
            guard isVisibleTab(nextId) else { continue }
            if let currentId = activeVisibleTab?.id {
                navigationBackStack.append(currentId)
            }
            activateTab(nextId)
            return
        }
    }

    private func isVisibleTab(_ tabId: String) -> Bool {
        visibleTabs.contains { $0.id == tabId }
    }

    private func activateTab(_ tabId: String) {
        guard let tab = tabs.first(where: { $0.id == tabId }) else { return }
        activeTabId = tab.id
        selectedItemId = tab.itemId
    }

    @discardableResult
    func closeActiveTab() -> Bool {
        guard let tab = activeVisibleTab else { return false }
        closeTab(tab)
        return true
    }

    func signOut() async {
        guard !isSigningOut else { return }
        isSigningOut = true
        let priorPhase = phase
        phase = .loading
        defer { isSigningOut = false }
        guard await flushPendingChanges() else {
            phase = priorPhase
            return
        }
        workspaceReloadGeneration = UUID()
        cancelPostReadyWork()
        Self.invalidateWorkspaceTransitionState(
            generation: &projectSelectionGeneration,
            loadingProjectId: &loadingProjectId,
            isSwitchingMemoryContext: &isSwitchingMemoryContext,
            isPreparingWorkspaceIndex: &isPreparingWorkspaceIndex,
            orgResourceRefreshGeneration: &orgResourceRefreshGeneration
        )
        _ = try? await server.raw(method: "DELETE", path: "/api/v1/auth/session")
        do {
            _ = try await projectSelectionSideEffectGate.run {
                try await daemon.replaceProjectConfig(
                    .init(
                        serverUrl: ClumsiesIdentifiers.serverURL.absoluteString,
                        projectId: nil,
                        accessToken: nil,
                        refreshToken: nil
                    )
                )
            }
            clearAuthorityScopedWorkspace()
            phase = .authenticationRequired
        } catch {
            errorMessage = error.localizedDescription
            phase = .failed(error.localizedDescription)
        }
    }

    func createMemory(kind: MemoryKind, scope: MemoryScope) async {
        do {
            _ = try await createMemoryDraft(kind: kind, scope: scope)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func createMemoryDraft(
        kind: MemoryKind,
        scope: MemoryScope
    ) async throws -> String? {
        guard scope == .org, canCreateMemory(kind: kind, scope: scope),
              let projectId = activeProjectId else { return nil }
        let generation = workspaceReloadGeneration
        guard let authority = try await loadStableOrgAuthoritySnapshot(
            allowingEmptyHead: true
        ) else {
            throw ServerClientError.invalidResponse(
                "A fresh Organization Memory snapshot is required to create a Draft."
            )
        }
        guard workspaceReloadGeneration == generation else { return nil }
        return try await withDraftMutation {
            guard Self.projectContextIsCurrent(
                isSwitchingMemoryContext: isSwitchingMemoryContext,
                activeProjectId: activeProjectId,
                expectedProjectId: projectId
            ), workspaceReloadGeneration == generation else { return nil }
            let path = uniqueDefaultPath(
                for: kind,
                scope: scope,
                authoritativeOrgResources: authority.resources,
                projectId: projectId
            )
            let document = Self.defaultDocument(kind: kind, path: path)
            let response = try await daemon.store(
                .init(
                    draftId: nil,
                    baseCommitId: authority.commitId,
                    projectId: projectId,
                    scope: .org,
                    resource: kind.daemonKind,
                    op: .create(
                        path: document.path,
                        content: daemonContent(kind: kind, document: document),
                        description: nil
                    ),
                    source: .desktop
                )
            )
            guard Self.projectContextIsCurrent(
                isSwitchingMemoryContext: isSwitchingMemoryContext,
                activeProjectId: activeProjectId,
                expectedProjectId: projectId
            ), workspaceReloadGeneration == generation else { return nil }
            try await refreshDraft(response.draftId)
            guard Self.projectContextIsCurrent(
                isSwitchingMemoryContext: isSwitchingMemoryContext,
                activeProjectId: activeProjectId,
                expectedProjectId: projectId
            ), workspaceReloadGeneration == generation else { return nil }
            selectedItemId = response.draftId
            return response.draftId
        }
    }

    /// Use loaded metadata for presentation; adoption revalidates shared authority before writing.
    func prepareMemoryGuidelines(
        projectId: String,
        refreshingAuthority: Bool = false
    ) async throws -> MemoryGuidelinesSetup {
        let generation = workspaceReloadGeneration
        let config = try await daemon.projectConfig()
        var authorityResources = resources.filter { $0.scope == .org }
        var authorityCommitId = orgRefCommitId
        if refreshingAuthority {
            guard let authority = try await loadStableOrgAuthoritySnapshot(allowingEmptyHead: true) else {
                throw ServerClientError.invalidResponse("Couldn’t check your organization's memory guidelines. Try again.")
            }
            authorityResources = authority.resources
            authorityCommitId = authority.commitId
            await refreshDraftInventory(includeFailed: true, generation: generation)
        }
        try Task.checkCancellation()
        guard generation == workspaceReloadGeneration, phase == .ready,
              selectedSection == .memory,
              Self.projectContextIsCurrent(
                  isSwitchingMemoryContext: isSwitchingMemoryContext,
                  activeProjectId: activeProjectId,
                  expectedProjectId: projectId
              ) else { throw CancellationError() }
        switch draftInventoryLoadState {
        case .loaded: break
        case .failed(let message): throw ServerClientError.invalidResponse(message)
        case .loading: throw ServerClientError.invalidResponse("Wait for drafts to finish loading, then try again.")
        }
        var setup = try MemoryGuidelines.setup(
            projectId: projectId,
            path: MemoryGuidelines.configuredPath(config.memoryGuidelinesPath),
            items: visibleMemoryItems,
            organizationResources: authorityResources
        )
        setup.organizationCommitId = authorityCommitId
        setup.occupiedPaths = Set(authorityResources.map(\.document.path))
            .union(visibleMemoryItems.map(\.document.path))
            .union(Self.memoryTreeDrafts(drafts, activeProjectId: projectId).map(\.document.path))
        return setup
    }

    /// Recheck the offered action. A changed destination is presented again for the user to choose.
    func useMemoryGuidelines(_ offered: MemoryGuidelinesSetup) async throws -> MemoryGuidelinesSetup {
        let current = try await prepareMemoryGuidelines(projectId: offered.projectId, refreshingAuthority: true)
        guard current.hasSameDestination(as: offered) else { return current }
        let generation = workspaceReloadGeneration
        let itemId: String
        switch current.action {
        case .open(let id):
            itemId = id
        case .useOrganization(let resource):
            if !resources.contains(where: { $0.id == resource.id }) {
                resources.append(resource)
            }
            try await addOrgMemories(resourceIds: [resource.id], toProject: current.projectId)
            itemId = resource.id
        case .createDefault:
            itemId = try await createMemoryGuidelines(current)
        }
        guard generation == workspaceReloadGeneration,
              activeProjectId == current.projectId,
              selectedSection == .memory else { throw CancellationError() }
        guard let item = visibleMemoryItems.first(where: { $0.id == itemId }) else {
            throw ServerClientError.invalidResponse("Memory guidelines were saved, but could not be opened. Refresh the project to open them.")
        }
        open(item, mode: .preview)
        return current
    }

    private func createMemoryGuidelines(_ setup: MemoryGuidelinesSetup) async throws -> String {
        let generation = workspaceReloadGeneration
        return try await withDraftMutation {
            guard generation == workspaceReloadGeneration,
                  activeProjectId == setup.projectId,
                  canCreateMemory(kind: .context, scope: .org) else { throw CancellationError() }
            let occupiedPaths = setup.occupiedPaths.union(
                Self.memoryTreeDrafts(drafts, activeProjectId: setup.projectId).map(\.document.path)
            )
            let documents = try MemoryGuidelines.defaultDocuments(occupiedPaths: occupiedPaths)
            for document in documents { try validate(kind: .context, document: document) }
            let responses = try await daemon.createMemoryDrafts(.init(
                projectId: setup.projectId,
                baseCommitId: setup.organizationCommitId,
                operations: documents.map {
                    .create(path: $0.path, content: daemonContent(kind: .context, document: $0), description: nil)
                }
            ))
            guard generation == workspaceReloadGeneration,
                  activeProjectId == setup.projectId else { throw CancellationError() }
            let daemon = daemon
            let details = try await concurrentMap(responses) { try await daemon.draft($0.draftId) }
            guard generation == workspaceReloadGeneration,
                  activeProjectId == setup.projectId,
                  let guideline = responses.first else { throw CancellationError() }
            let created = details.map { WorkspaceLoader.mapDraft($0, resources: resources) }
            let createdIds = Set(created.map(\.id))
            drafts = drafts.filter { !createdIds.contains($0.id) } + created
            selectedItemId = guideline.draftId
            return guideline.draftId
        }
    }

    func stageDocumentSave(_ item: MemoryListItem, document: EditableMemoryDocument) {
        guard phase == .ready, !isSigningOut else { return }
        guard canEditMemory(item) else {
            errorMessage = "You do not have permission to edit this memory."
            return
        }
        guard synchronizationItemId(for: item) == nil else {
            errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
            return
        }
        guard !isSwitchingMemoryContext else {
            errorMessage = "Wait for the Memory context switch to finish before editing."
            return
        }
        guard let key = documentSessionKey(for: item) else {
            errorMessage = "Open this memory from a Project before editing it."
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
        guard let key = documentSessionKey(for: item) else { return }
        try await flushDocumentSave(key)
    }

    private func flushDocumentSave(_ key: MemoryDocumentSessionKey) async throws {
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
        guard let key = documentSessionKey(for: item) else { return }
        cancelDocumentSave(key)
    }

    private func cancelDocumentSave(_ key: MemoryDocumentSessionKey) {
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

    func stageBundleSave(
        _ bundle: PersonalBundle,
        name: String,
        description: String,
        resourceIds: Set<String>
    ) {
        guard phase == .ready, !isSigningOut else { return }
        let generation = UUID()
        pendingBundleSaves[bundle.id] = .init(
            bundle: bundle,
            name: name,
            description: description,
            resourceIds: resourceIds,
            generation: generation
        )
        bundleSaveTasks[bundle.id]?.cancel()
        bundleSaveTasks[bundle.id] = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(600))
            guard !Task.isCancelled else { return }
            await self?.persistBundleSave(bundle.id, generation: generation)
        }
    }

    func flushBundleSave(_ bundleId: String) async throws {
        bundleSaveTasks[bundleId]?.cancel()
        bundleSaveTasks[bundleId] = nil
        guard let pending = pendingBundleSaves[bundleId] else { return }
        try await updateBundle(
            pending.bundle,
            name: pending.name,
            description: pending.description,
            resourceIds: pending.resourceIds
        )
        if pendingBundleSaves[bundleId]?.generation == pending.generation {
            pendingBundleSaves[bundleId] = nil
        }
    }

    func cancelBundleSave(_ bundleId: String) {
        bundleSaveTasks[bundleId]?.cancel()
        bundleSaveTasks[bundleId] = nil
        pendingBundleSaves[bundleId] = nil
    }

    func flushPendingChanges() async -> Bool {
        guard await flushPendingDocumentChanges() else { return false }
        do {
            for bundleId in Array(pendingBundleSaves.keys) {
                try await flushBundleSave(bundleId)
            }
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }

    private func flushPendingDocumentChanges() async -> Bool {
        do {
            for key in Array(pendingDocumentSaves.keys) {
                try await flushDocumentSave(key)
            }
            return true
        } catch {
            errorMessage = error.localizedDescription
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
            && item.projectContextId.map(projectOrgSelectionMutatingIds.contains) == true
        guard canEditMemory(item) || flushesSelectionMutation else {
            throw ServerClientError.forbidden("You do not have permission to edit this memory.")
        }
        if isSwitchingMemoryContext, pendingSaveKey == nil {
            throw ServerClientError.forbidden(
                "Wait for the Memory context switch to finish before editing."
            )
        }
        if !allowingDuringSynchronization,
           synchronizationItemId(for: item) != nil {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        try validate(kind: item.kind, document: document)
        try await withDraftMutation {
            if let pendingSaveKey, let pendingSaveGeneration {
                guard pendingDocumentSaves[pendingSaveKey]?.generation
                    == pendingSaveGeneration else { return }
            }
            if !allowingDuringSynchronization,
               synchronizationItemId(for: item) != nil {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            let resource = item.resource
            let draft = currentDraft(for: item)
            guard let projectId = draftCarrierProjectId(for: item, currentDraft: draft) else {
                throw WorkspaceLoadError.noProjects
            }
            if item.draft != nil, draft == nil {
                finishPendingDocumentSaveIfCurrent(
                    key: pendingSaveKey,
                    generation: pendingSaveGeneration
                )
                return
            }
            if draft?.isDeletion == true {
                finishPendingDocumentSaveIfCurrent(
                    key: pendingSaveKey,
                    generation: pendingSaveGeneration
                )
                return
            }
            let draftId = draft?.id
            let projectRefCommitId = projects.first { $0.id == projectId }?.refCommitId
            let baseCommitId = draft?.baseCommitId
                ?? resource?.refCommitId
                ?? (item.scope == .org ? orgRefCommitId : projectRefCommitId)
            var response: DaemonDraftOperationResponse?

            if let resource, document.path != (draft?.document.path ?? resource.document.path) {
                response = try await daemon.store(
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
                    content: daemonContent(kind: item.kind, document: document),
                    description: nil
                )
            } else {
                operation = .create(
                    path: document.path,
                    content: daemonContent(kind: item.kind, document: document),
                    description: nil
                )
            }
            response = try await daemon.store(
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
                try await refreshDraft(response.draftId)
                if activeProjectId == projectId {
                    selectedItemId = resource?.id ?? response.draftId
                }
            }
            finishPendingDocumentSaveIfCurrent(
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
        guard !isSwitchingMemoryContext else {
            throw ServerClientError.forbidden(
                "Wait for the Memory context switch to finish before renaming."
            )
        }
        guard canEditMemory(item) else {
            throw ServerClientError.forbidden("You do not have permission to rename this memory.")
        }
        guard let sessionKey = documentSessionKey(for: item) else {
            throw ServerClientError.forbidden(
                "Open this memory from a Project before renaming it."
            )
        }
        guard synchronizationItemId(for: item) == nil else {
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
            guard synchronizationItemId(for: item) == nil else {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            let resource = item.resource
            let draft = currentDraft(for: item)
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

            let projectRefCommitId = projects.first { $0.id == projectId }?.refCommitId
            let response = try await daemon.store(
                .init(
                    draftId: draft?.id,
                    baseCommitId: draft?.baseCommitId
                        ?? resource?.refCommitId
                        ?? (item.scope == .org ? orgRefCommitId : projectRefCommitId),
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
            retargetPendingDocumentSave(sessionKey, to: newPath)
            try await refreshDraft(response.draftId)
            retargetPendingDocumentSave(sessionKey, to: newPath)
            if activeProjectId == sessionKey.projectId {
                selectedItemId = resource?.id ?? plan.targetId
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
        guard !isSwitchingMemoryContext else {
            errorMessage = "Wait for the Memory context switch to finish before deleting."
            return false
        }
        guard canEditMemory(item) else {
            errorMessage = "You do not have permission to delete this memory."
            return false
        }
        guard synchronizationItemId(for: item) == nil else {
            errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
            return false
        }
        cancelDocumentSave(item)
        guard let projectId = draftCarrierProjectId(for: item, currentDraft: item.draft),
              let targetId = item.resource?.id ?? item.draft?.targetId else { return false }
        do {
            try await withDraftMutation {
                guard synchronizationItemId(for: item) == nil else {
                    throw DocumentSyncError.mutationWhileSynchronizing
                }
                let draft = currentDraft(for: item)
                let response = try await daemon.store(
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
                try await refreshDraft(response.draftId)
            }
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }

    @discardableResult
    func discard(_ draft: LocalDraft) async -> Bool {
        guard synchronizationItemId(for: draft) == nil else {
            errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
            return false
        }
        cancelDocumentSave(
            .init(projectId: draft.projectId, itemId: draft.targetId ?? draft.id)
        )
        do {
            try await withDraftMutation {
                guard synchronizationItemId(for: draft) == nil else {
                    throw DocumentSyncError.mutationWhileSynchronizing
                }
                guard drafts.contains(where: { $0.id == draft.id }) else { return }
                _ = try await daemon.store(
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
                drafts.removeAll { $0.id == draft.id }
                pruneOrphanedMemoryTabs()
                selectedItemId = nil
            }
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }

    @discardableResult
    func retrySync(
        channel: String = "all",
        projectId: String? = nil
    ) async -> SyncRetryOutcome {
        let projectId = projectId ?? activeProjectId
        let key = SyncRetryKey(channel: channel, projectId: projectId)
        if let inFlight = syncRetryTasks[key] {
            return await inFlight.task.value
        }

        clearSyncRetryErrors(channel: channel, projectId: projectId)
        let predecessors = syncRetryTasks.compactMap { existingKey, handle in
            existingKey.projectId == projectId ? handle.task : nil
        }

        let taskId = UUID()
        let task = Task { @MainActor in
            do {
                for predecessor in predecessors {
                    _ = await predecessor.value
                    try Task.checkCancellation()
                }
                clearSyncRetryErrors(channel: channel, projectId: projectId)
                try Task.checkCancellation()
                _ = try await daemon.retrySync(channel: channel, projectId: projectId)
                try Task.checkCancellation()
                if channel == "all" {
                    clearSyncRetryErrors(channel: channel, projectId: projectId)
                }
                if activeProjectId == projectId {
                    await refreshSyncStatus()
                    try Task.checkCancellation()
                    await refreshSynchronizedWorkspaceData()
                    try Task.checkCancellation()
                }
                return SyncRetryOutcome.completed
            } catch is CancellationError {
                return .cancelled
            } catch {
                guard !Task.isCancelled else { return .cancelled }
                let message = error.localizedDescription
                syncRetryErrors[key] = message
                if activeProjectId == projectId {
                    syncStatusAvailable = false
                    if errorMessage == nil {
                        errorMessage = message
                        presentedSyncRetryErrorKey = key
                    }
                }
                return .failed(message)
            }
        }
        syncRetryTasks[key] = .init(id: taskId, task: task)
        retryingSyncKeys.insert(key)
        let outcome = await task.value
        if syncRetryTasks[key]?.id == taskId {
            syncRetryTasks[key] = nil
            retryingSyncKeys.remove(key)
        }
        return outcome
    }

    func runRefreshLoop() async {
        let clock = ContinuousClock()
        var nextSynchronizedDataRefresh = clock.now
        while !Task.isCancelled {
            if phase == .ready {
                await refreshSyncStatus()
                guard !Task.isCancelled else { return }
                if clock.now >= nextSynchronizedDataRefresh {
                    await refreshSynchronizedWorkspaceData()
                    nextSynchronizedDataRefresh = clock.now.advanced(
                        by: WorkspaceRefreshCadence.synchronizedData
                    )
                }
            }
            do {
                try await Task.sleep(for: WorkspaceRefreshCadence.syncStatus)
            } catch {
                return
            }
        }
    }

    func refreshSyncStatus() async {
        guard phase == .ready, !isRefreshingSyncStatus else { return }
        isRefreshingSyncStatus = true
        defer { isRefreshingSyncStatus = false }
        let generation = workspaceReloadGeneration
        let projectId = activeProjectId
        guard let runtime else { return }
        do {
            let sync = try await daemon.syncStatus(projectId: projectId)
            try Task.checkCancellation()
            guard workspaceReloadGeneration == generation,
                  activeProjectId == projectId,
                  phase == .ready else {
                return
            }
            let updatedRuntime = RuntimeState(
                health: runtime.health,
                sync: sync,
                serverDataSource: server.dataSource
            )
            if self.runtime != updatedRuntime {
                self.runtime = updatedRuntime
            }
            if !syncStatusAvailable {
                syncStatusAvailable = true
            }
        } catch is CancellationError {
            return
        } catch {
            guard workspaceReloadGeneration == generation,
                  activeProjectId == projectId else {
                return
            }
            if syncStatusAvailable {
                syncStatusAvailable = false
            }
        }
    }

    func refreshSynchronizedWorkspaceData() async {
        guard phase == .ready, !isRefreshingSynchronizedWorkspaceData else { return }
        isRefreshingSynchronizedWorkspaceData = true
        defer { isRefreshingSynchronizedWorkspaceData = false }
        let generation = workspaceReloadGeneration
        let projectId = activeProjectId
        await refreshOrgResourcesIfNeeded()
        guard workspaceReloadGeneration == generation,
              activeProjectId == projectId,
              phase == .ready,
              !Task.isCancelled,
              let sync = runtime?.sync else {
            return
        }
        if draftInventoryLoadTask == nil {
            await refreshDraftInventory(
                includeFailed: sync.pendingOperationCount > 0
                    || sync.failedOperationCount > 0,
                generation: generation
            )
        }
        guard workspaceReloadGeneration == generation,
              activeProjectId == projectId,
              phase == .ready,
              !Task.isCancelled else {
            return
        }
        await refreshStaleResourcesIfNeeded(sync: sync)
    }

    nonisolated static func stableOrgAuthorityCommitId(
        beforeCommitId: String?,
        afterCommitId: String?,
        responseIsStale: Bool
    ) -> String? {
        guard !responseIsStale,
              let beforeCommitId,
              afterCommitId == beforeCommitId else {
            return nil
        }
        return beforeCommitId
    }

    private func loadStableOrgAuthoritySnapshot(
        allowingEmptyHead: Bool = false
    ) async throws
        -> (commitId: String?, refEtag: String?, resources: [MemoryResource])? {
        let before: (value: CommitStateResponse, response: DaemonServerResponse) =
            try await server.getWithMetadata("/api/v1/org/commit-state")

        var metadata: [MemoryMetadata] = []
        var cursor: String?
        var listingIsStale = false
        repeat {
            var query = [URLQueryItem(name: "limit", value: "200")]
            if let cursor { query.append(.init(name: "cursor", value: cursor)) }
            let page: (value: ListResponse<MemoryMetadata>, response: DaemonServerResponse) =
                try await server.getWithMetadata("/api/v1/org/memories", query: query)
            listingIsStale = listingIsStale || page.response.isStaleCache
            metadata += page.value.items
            cursor = page.value.pageInfo.hasMore ? page.value.pageInfo.nextCursor : nil
        } while cursor != nil

        let after: (value: CommitStateResponse, response: DaemonServerResponse) =
            try await server.getWithMetadata("/api/v1/org/commit-state")
        let responseIsStale = before.response.isStaleCache
            || listingIsStale
            || after.response.isStaleCache
        let commitId = before.value.ref.commitId
        guard !responseIsStale,
              after.value.ref.commitId == commitId,
              allowingEmptyHead || commitId != nil else {
            return nil
        }
        let refEtag = after.response.headers.first {
            $0.key.caseInsensitiveCompare("etag") == .orderedSame
        }?.value ?? before.response.headers.first {
            $0.key.caseInsensitiveCompare("etag") == .orderedSame
        }?.value
        return (
            commitId,
            refEtag,
            metadata.map { item in
                MemoryResource(
                    id: item.memoryId,
                    scope: .org,
                    projectId: nil,
                    projectName: nil,
                    kind: .init(.memory),
                    contentHash: item.contentHash,
                    updatedAt: item.updatedAt,
                    refCommitId: commitId,
                    contentLoaded: false,
                    document: .init(title: item.name, path: item.path, body: "")
                )
            }
        )
    }

    nonisolated static func reconciledOrgResources(
        existing: [MemoryResource],
        authoritative: [MemoryResource]
    ) -> [MemoryResource] {
        let existingById = Dictionary(
            existing.lazy.filter { $0.scope == .org }.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        return authoritative.filter { $0.scope == .org }.map { authority in
            guard let current = existingById[authority.id],
                  current.kind == authority.kind,
                  current.document.path == authority.document.path,
                  current.contentHash == authority.contentHash,
                  current.contentLoaded,
                  Self.contentHash(current.document.body) == current.contentHash else {
                return authority
            }
            var preserved = authority
            preserved.contentLoaded = true
            preserved.document.body = current.document.body
            return preserved
        }
    }

    private func refreshOrgResourcesIfNeeded() async {
        let workspaceGeneration = workspaceReloadGeneration
        guard selectedSection == .memory,
              activeProjectId == nil,
              !isSwitchingMemoryContext,
              orgResourceRefreshGeneration == nil else {
            return
        }
        let observedOrgRefCommitId = orgRefCommitId
        let generation = UUID()
        orgResourceRefreshGeneration = generation
        defer {
            if orgResourceRefreshGeneration == generation {
                orgResourceRefreshGeneration = nil
            }
        }
        do {
            let head: (value: CommitStateResponse, response: DaemonServerResponse) =
                try await server.getWithMetadata("/api/v1/org/commit-state")
            guard workspaceReloadGeneration == workspaceGeneration,
                  phase == .ready,
                  selectedSection == .memory,
                  activeProjectId == nil,
                  !isSwitchingMemoryContext,
                  orgResourceRefreshGeneration == generation,
                  !head.response.isStaleCache else {
                return
            }
            guard head.value.ref.commitId != observedOrgRefCommitId else {
                resolveBackgroundError(.organizationResources)
                return
            }
            guard let snapshot = try await loadStableOrgAuthoritySnapshot(),
                  let snapshotCommitId = snapshot.commitId,
                  snapshotCommitId == head.value.ref.commitId,
                  workspaceReloadGeneration == workspaceGeneration,
                  phase == .ready,
                  selectedSection == .memory,
                  activeProjectId == nil,
                  !isSwitchingMemoryContext,
                  orgResourceRefreshGeneration == generation,
                  orgRefCommitId == observedOrgRefCommitId else {
                return
            }

            // Any inactive Project plan containing an Org row was derived
            // from an older global authority generation. Drop the whole plan
            // before installing the new Org snapshot so returning to that
            // Project can never apply an old checkout and advance its ref.
            let projectsWithOrgStalePlans = Set(staleResourceSnapshots.values.compactMap {
                snapshot in
                snapshot.local?.scope == .org || snapshot.remote?.scope == .org
                    ? snapshot.projectId
                    : nil
            })
            for projectId in projectsWithOrgStalePlans {
                clearStaleResourceState(for: projectId)
            }
            let previousOrgResources = resources.filter { $0.scope == .org }
            let nextOrgResources = Self.reconciledOrgResources(
                existing: previousOrgResources,
                authoritative: snapshot.resources
            )
            let previousById = Dictionary(
                previousOrgResources.map { ($0.id, $0) },
                uniquingKeysWith: { _, latest in latest }
            )
            let nextById = Dictionary(
                nextOrgResources.map { ($0.id, $0) },
                uniquingKeysWith: { _, latest in latest }
            )

            // Publish the complete authority generation in one assignment so
            // the tree cannot observe a mixed old/new Org catalog.
            resources = resources.filter { $0.scope != .org } + nextOrgResources
            orgRefCommitId = snapshotCommitId
            orgRefEtag = snapshot.refEtag ?? ""
            for resourceId in Set(previousById.keys).union(nextById.keys) {
                let previous = previousById[resourceId]
                let next = nextById[resourceId]
                if previous?.kind != next?.kind
                    || previous?.contentHash != next?.contentHash
                    || previous?.contentLoaded != next?.contentLoaded
                    || previous?.document != next?.document {
                    bumpDocumentContentGeneration(for: resourceId)
                }
            }
            if let selectedItemId,
               previousById[selectedItemId] != nil,
               nextById[selectedItemId] == nil {
                self.selectedItemId = nil
            }
            pruneOrphanedMemoryTabs()
            refreshAllDocumentTabs()
            resolveBackgroundError(.organizationResources)
        } catch is CancellationError {
            return
        } catch {
            guard workspaceReloadGeneration == workspaceGeneration,
                  phase == .ready,
                  selectedSection == .memory,
                  activeProjectId == nil,
                  !isSwitchingMemoryContext,
                  orgResourceRefreshGeneration == generation else {
                return
            }
            presentBackgroundError(
                "Couldn’t refresh Organization Memory. Existing content is still available. "
                    + error.localizedDescription,
                source: .organizationResources
            )
        }
    }

    nonisolated static func staleResourcePlan(
        displayedResources: [MemoryResource],
        projectName: String,
        observedProjectRefCommitId: String?,
        observedSelectedOrgResourceIds: Set<String> = [],
        observedOrgSelectionRevision: Int = 0,
        authoritativeCommitId: String?,
        serverCursor: String?,
        checkout: DaemonProjectCheckout,
        authoritativeRefEtag: String? = nil,
        authoritativeResponseIsStale: Bool = false,
        authoritativeOrgResources: [MemoryResource]? = nil,
        authoritativeOrgRefCommitId: String? = nil,
        authoritativeOrgResponseIsStale: Bool = false,
        provisionalResourceIds: Set<String> = [],
        generation: UUID = UUID()
    ) -> [String: StaleResourceSyncSnapshot]? {
        // A cursor mismatch has no direction information. Only a fresh Server
        // commit-state response can prove that the installed checkout is the
        // current shared version rather than an older checkout catching up.
        guard !authoritativeResponseIsStale,
              checkout.ready,
              let authoritativeCommitId,
              serverCursor == authoritativeCommitId,
              checkout.commitId == authoritativeCommitId else {
            return nil
        }
        guard observedProjectRefCommitId != authoritativeCommitId else { return [:] }

        let projectId = checkout.projectId
        let localProjectResources = displayedResources.filter {
            $0.scope == .project
                && $0.projectId == projectId
                && !provisionalResourceIds.contains($0.id)
        }
        let selectedOrgResourceIds = Set(checkout.selectedOrgResourceIds)
        let checkoutOrgResources = checkout.resources.filter { $0.scope == .org }
        let checkoutOrgResourceIds = Set(checkoutOrgResources.map(\.resourceId))
        guard checkoutOrgResourceIds == selectedOrgResourceIds else { return nil }

        // A Project checkout proves which Org blobs were materialized for that
        // Project, but absence from the checkout can mean either an Org delete
        // or a harmless Project deselection. Only a fresh, stable Org listing
        // can distinguish those cases and prove the checkout body still points
        // forward to the current Org authority.
        let orgIdsRequiringAuthority = observedSelectedOrgResourceIds
            .union(selectedOrgResourceIds)
        let authoritativeOrgById: [String: MemoryResource]
        if orgIdsRequiringAuthority.isEmpty {
            authoritativeOrgById = [:]
        } else {
            guard !authoritativeOrgResponseIsStale,
                  let authoritativeOrgResources,
                  let authoritativeOrgRefCommitId else {
                return nil
            }
            authoritativeOrgById = Dictionary(
                authoritativeOrgResources.compactMap { resource in
                    guard resource.scope == .org,
                          resource.projectId == nil,
                          resource.refCommitId == authoritativeOrgRefCommitId else {
                        return nil
                    }
                    return (resource.id, resource)
                },
                uniquingKeysWith: { _, latest in latest }
            )
        }

        let checkoutOrgById = Dictionary(
            checkoutOrgResources.map { ($0.resourceId, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        for resourceId in selectedOrgResourceIds {
            guard let checkoutResource = checkoutOrgById[resourceId],
                  let authority = authoritativeOrgById[resourceId],
                  authority.kind == .init(checkoutResource.resourceKind),
                  authority.document.path == checkoutResource.path,
                  authority.contentHash == checkoutResource.contentHash,
                  Self.contentHash(checkoutResource.content.content)
                    == checkoutResource.contentHash else {
                return nil
            }
        }

        // A resource removed from Project selection remains Org authority and
        // must stay in the global collection. It becomes a deletion candidate
        // only when the authoritative Org listing also says it is gone.
        let deletedSelectedOrgResourceIds = observedSelectedOrgResourceIds
            .subtracting(selectedOrgResourceIds)
            .filter { authoritativeOrgById[$0] == nil }
        let relevantOrgResourceIds = selectedOrgResourceIds
            .union(deletedSelectedOrgResourceIds)
        let localOrgResources = displayedResources.filter {
            $0.scope == .org
                && relevantOrgResourceIds.contains($0.id)
                && !provisionalResourceIds.contains($0.id)
        }
        let localResources = localProjectResources + localOrgResources
        let checkoutResources = checkout.resources.filter { $0.scope == .project }
        let localById = Dictionary(
            localResources.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        var remoteById = Dictionary(
            checkoutResources.map { resource -> (String, MemoryResource) in
                let local = localById[resource.resourceId]
                return (
                    resource.resourceId,
                    MemoryResource(
                        id: resource.resourceId,
                        scope: .project,
                        projectId: projectId,
                        projectName: projectName,
                        kind: .init(resource.resourceKind),
                        contentHash: resource.contentHash,
                        updatedAt: checkout.commitCreatedAt ?? local?.updatedAt ?? "",
                        refCommitId: authoritativeCommitId,
                        contentLoaded: true,
                        document: .init(
                            title: URL(fileURLWithPath: resource.path)
                                .deletingPathExtension().lastPathComponent,
                            path: resource.path,
                            body: resource.content.content
                        )
                    )
                )
            },
            uniquingKeysWith: { _, latest in latest }
        )
        for resourceId in selectedOrgResourceIds {
            guard let checkoutResource = checkoutOrgById[resourceId],
                  let authority = authoritativeOrgById[resourceId] else {
                return nil
            }
            remoteById[resourceId] = MemoryResource(
                id: resourceId,
                scope: .org,
                projectId: nil,
                projectName: nil,
                kind: authority.kind,
                contentHash: authority.contentHash,
                updatedAt: authority.updatedAt,
                refCommitId: authority.refCommitId,
                contentLoaded: true,
                document: .init(
                    title: authority.document.title,
                    path: authority.document.path,
                    body: checkoutResource.content.content
                )
            )
        }
        let projectRemoteIds = Set(checkoutResources.map(\.resourceId))
        let ids = Set(localResources.map(\.id))
            .union(projectRemoteIds)
            .union(selectedOrgResourceIds)
        var result: [String: StaleResourceSyncSnapshot] = [:]
        for id in ids {
            let local = localById[id]
            let remote = remoteById[id]
            if let local, let remote,
               local.scope == remote.scope,
               local.kind == remote.kind,
               local.document.path == remote.document.path,
               local.contentHash == remote.contentHash,
               (local.scope == .project
                    || !local.contentLoaded
                    || Self.contentHash(local.document.body) == local.contentHash) {
                continue
            }
            result[id] = .init(
                projectId: projectId,
                observedProjectRefCommitId: observedProjectRefCommitId,
                observedSelectedOrgResourceIds: observedSelectedOrgResourceIds,
                observedOrgSelectionRevision: observedOrgSelectionRevision,
                authoritativeCommitId: authoritativeCommitId,
                authoritativeRefEtag: authoritativeRefEtag ?? checkout.refEtag,
                selectedOrgResourceIds: Set(checkout.selectedOrgResourceIds),
                orgSelectionRevision: checkout.orgSelectionRevision,
                generation: generation,
                local: local,
                remote: remote
            )
        }
        return result
    }

    nonisolated static func draftUploadBarrierDecision(
        serverDraftId: String?,
        pendingOperationCount: Int,
        failedOperationCount: Int,
        operationStates: [DaemonDraftSyncState],
        failureMessage: String?
    ) -> DraftUploadBarrierDecision {
        if failedOperationCount > 0 || operationStates.contains(.failed) {
            return .failed(failureMessage)
        }
        if pendingOperationCount == 0,
           serverDraftId != nil,
           operationStates.allSatisfy({ $0 == .synced }) {
            return .ready
        }
        return .wait
    }

    nonisolated static func staleResourcePlansMatch(
        _ lhs: [String: StaleResourceSyncSnapshot],
        _ rhs: [String: StaleResourceSyncSnapshot]
    ) -> Bool {
        guard Set(lhs.keys) == Set(rhs.keys) else { return false }
        return lhs.allSatisfy { resourceId, left in
            guard let right = rhs[resourceId],
                  left.local?.contentLoaded != false,
                  right.local?.contentLoaded != false else { return false }
            return left.projectId == right.projectId
                && left.observedProjectRefCommitId == right.observedProjectRefCommitId
                && left.observedSelectedOrgResourceIds == right.observedSelectedOrgResourceIds
                && left.observedOrgSelectionRevision == right.observedOrgSelectionRevision
                && left.authoritativeCommitId == right.authoritativeCommitId
                && left.authoritativeRefEtag == right.authoritativeRefEtag
                && left.selectedOrgResourceIds == right.selectedOrgResourceIds
                && left.orgSelectionRevision == right.orgSelectionRevision
                && left.local == right.local
                && left.remote == right.remote
        }
    }

    private func refreshStaleResourcesIfNeeded(sync: DaemonSyncStatus) async {
        let workspaceGeneration = workspaceReloadGeneration
        guard let projectId = activeProjectId,
              let project = projects.first(where: { $0.id == projectId }),
              let serverCursor = sync.commitSync.serverCursor else {
            return
        }
        let errorSource = WorkspaceBackgroundErrorSource.staleResources(projectId: projectId)
        guard serverCursor != project.refCommitId else {
            resolveBackgroundError(errorSource)
            return
        }
        let observedRef = project.refCommitId
        let refreshGeneration = UUID()
        staleResourceRefreshGenerations[projectId] = refreshGeneration
        do {
            let commit: (value: CommitStateResponse, response: DaemonServerResponse) =
                try await server.getWithMetadata("/api/v1/projects/\(projectId)/commit-state")
            guard activeProjectId == projectId,
                  projects.first(where: { $0.id == projectId }) == project,
                  staleResourceRefreshGenerations[projectId] == refreshGeneration,
                  !commit.response.isStaleCache else {
                return
            }
            let authoritativeCommitId = commit.value.ref.commitId
            let authoritativeRefEtag = commit.response.headers.first {
                $0.key.caseInsensitiveCompare("etag") == .orderedSame
            }?.value
            if authoritativeCommitId == observedRef {
                clearStaleResourceState(for: projectId)
                if let authoritativeCommitId {
                    advanceProjectRefIfPlanCompleted(
                        projectId: projectId,
                        authoritativeCommitId: authoritativeCommitId,
                        authoritativeRefEtag: authoritativeRefEtag,
                        selectedOrgResourceIds: project.selectedOrgResourceIds,
                        orgSelectionRevision: project.orgSelectionRevision
                    )
                }
                resolveBackgroundError(errorSource)
                return
            }
            let checkout = try await daemon.projectCheckout(projectId)
            guard activeProjectId == projectId,
                  projects.first(where: { $0.id == projectId }) == project,
                  staleResourceRefreshGenerations[projectId] == refreshGeneration else {
                return
            }
            let needsOrgAuthority = !project.selectedOrgResourceIds.isEmpty
                || !checkout.selectedOrgResourceIds.isEmpty
            let orgAuthority: (
                commitId: String,
                refEtag: String?,
                resources: [MemoryResource]
            )?
            if needsOrgAuthority {
                guard let snapshot = try await loadStableOrgAuthoritySnapshot(),
                      let commitId = snapshot.commitId else { return }
                orgAuthority = (commitId, snapshot.refEtag, snapshot.resources)
            } else {
                orgAuthority = nil
            }
            let verifiedCommit: (value: CommitStateResponse, response: DaemonServerResponse) =
                try await server.getWithMetadata(
                    "/api/v1/projects/\(projectId)/commit-state"
                )
            guard activeProjectId == projectId,
                  projects.first(where: { $0.id == projectId }) == project,
                  staleResourceRefreshGenerations[projectId] == refreshGeneration,
                  !verifiedCommit.response.isStaleCache,
                  verifiedCommit.value.ref.commitId == authoritativeCommitId,
                  let plan = Self.staleResourcePlan(
                    displayedResources: resources,
                    projectName: project.name,
                    observedProjectRefCommitId: observedRef,
                    observedSelectedOrgResourceIds: project.selectedOrgResourceIds,
                    observedOrgSelectionRevision: project.orgSelectionRevision,
                    authoritativeCommitId: authoritativeCommitId,
                    serverCursor: serverCursor,
                    checkout: checkout,
                    authoritativeRefEtag: authoritativeRefEtag,
                    authoritativeResponseIsStale: commit.response.isStaleCache,
                    authoritativeOrgResources: orgAuthority?.resources,
                    authoritativeOrgRefCommitId: orgAuthority?.commitId,
                    // Remote-only additions are inserted provisionally so the
                    // file tree can expose their Sync action. They are not
                    // part of the observed local generation and must not make
                    // the next poll conclude that the plan is already applied.
                    provisionalResourceIds: provisionalStaleAdditionIds
                  ) else {
                return
            }
            let installedPlan = staleResourceSnapshots.filter { _, snapshot in
                snapshot.projectId == projectId
            }
            if !plan.isEmpty, Self.staleResourcePlansMatch(plan, installedPlan) {
                resolveBackgroundError(errorSource)
                return
            }
            let hydratedPlan = await hydrateStaleResourcePlan(plan)
            guard activeProjectId == projectId,
                  projects.first(where: { $0.id == projectId }) == project,
                  staleResourceRefreshGenerations[projectId] == refreshGeneration else {
                return
            }
            installStaleResourcePlan(hydratedPlan, for: projectId)
            if hydratedPlan.isEmpty {
                advanceProjectRefIfPlanCompleted(
                    projectId: projectId,
                    authoritativeCommitId: authoritativeCommitId ?? serverCursor,
                    authoritativeRefEtag: authoritativeRefEtag ?? checkout.refEtag,
                    selectedOrgResourceIds: Set(checkout.selectedOrgResourceIds),
                    orgSelectionRevision: checkout.orgSelectionRevision
                )
            }
            resolveBackgroundError(errorSource)
        } catch is CancellationError {
            return
        } catch {
            guard workspaceReloadGeneration == workspaceGeneration,
                  phase == .ready,
                  activeProjectId == projectId,
                  projects.first(where: { $0.id == projectId }) == project,
                  staleResourceRefreshGenerations[projectId] == refreshGeneration else {
                return
            }
            presentBackgroundError(
                "Couldn’t update from the shared version. Existing content is unchanged. "
                    + error.localizedDescription,
                source: errorSource
            )
        }
    }

    /// Pulls the latest shared version for one document:
    /// - a behind draft opens the shared-change review flow;
    /// - a stale resource (no local draft) is refreshed from the synced checkout.
    func syncDocument(_ item: MemoryListItem) {
        guard Self.canStartDocumentSynchronization(
            isSwitchingMemoryContext: isSwitchingMemoryContext,
            activeProjectId: activeProjectId,
            itemProjectContextId: item.projectContextId
        ), let key = documentSessionKey(for: item) else {
            errorMessage = "Wait for the Memory context switch to finish before syncing this document."
            return
        }
        if projectOrgSelectionMutatingIds.contains(key.projectId) {
            errorMessage = "Wait for the Project memory selection to finish updating before syncing this document."
            return
        }
        let behindDraft = item.draft.flatMap { $0.freshness == .behind ? $0 : nil }
        let hasStaleResource = item.resource.map { resource in
            staleResourceSnapshots[resource.id]?.projectId == key.projectId
        } ?? false
        guard behindDraft != nil || hasStaleResource,
              synchronizingDocumentSessions.insert(key).inserted else { return }

        let generation = UUID()
        documentSynchronizationGenerations[key] = generation
        if behindDraft != nil {
            openDocumentForSync(item)
        }
        documentSynchronizationTasks[key] = Task { @MainActor [weak self] in
            guard let self else { return }
            if let behindDraft {
                await prepareBehindDraftSync(
                    key: key,
                    draft: behindDraft,
                    generation: generation
                )
            } else {
                await syncStaleResource(item, key: key, generation: generation)
            }
        }
    }

    private func openDocumentForSync(_ item: MemoryListItem) {
        let tabProjectId = item.projectContextId ?? (item.scope == .org ? nil : item.projectId)
        let existingMode = tabs.first {
            $0.section == selectedSection
                && $0.projectId == tabProjectId
                && $0.itemId == item.id
        }?.mode
        // The command is consumed by a DocumentSession. Keep an existing
        // Source/Diff mode, and use Source for a newly opened document so Sync
        // never silently switches the user to the default Preview.
        open(item, mode: existingMode ?? .source)
    }

    private func syncStaleResource(
        _ item: MemoryListItem,
        key: MemoryDocumentSessionKey,
        generation: UUID
    ) async {
        guard let resourceId = item.resource?.id else { return }
        guard isCurrentDocumentSynchronization(key, generation: generation) else { return }
        do {
            // A debounce save may not have materialized its draft yet. Flush
            // before applying a remote deletion/update so that local text can
            // enter reconciliation instead of becoming an invisible orphan.
            try await flushDocumentSave(key)
            guard isCurrentDocumentSynchronization(key, generation: generation) else { return }
            if let draft = currentDraft(for: item) {
                openDocumentForSync(item)
                await prepareBehindDraftSync(
                    key: key,
                    draft: draft,
                    generation: generation
                )
                return
            }
        } catch is CancellationError {
            endDocumentSynchronization(key, generation: generation)
            return
        } catch {
            guard isCurrentDocumentSynchronization(key, generation: generation) else { return }
            endDocumentSynchronization(key, generation: generation)
            errorMessage = error.localizedDescription
            return
        }

        // A repeated click after the first task completed is already satisfied.
        guard let snapshot = staleResourceSnapshots[resourceId],
              snapshot.projectId == key.projectId else {
            endDocumentSynchronization(key, generation: generation)
            return
        }
        guard projects.first(where: { $0.id == snapshot.projectId })?.refCommitId
            == snapshot.observedProjectRefCommitId,
              projects.first(where: { $0.id == snapshot.projectId })?.selectedOrgResourceIds
            == snapshot.observedSelectedOrgResourceIds,
              projects.first(where: { $0.id == snapshot.projectId })?.orgSelectionRevision
            == snapshot.observedOrgSelectionRevision else {
            endDocumentSynchronization(key, generation: generation)
            errorMessage = DocumentSyncError.checkoutNoLongerCurrent.localizedDescription
            return
        }
        if let remote = snapshot.remote {
            staleResourceRefreshGenerations[snapshot.projectId] = UUID()
            if let index = resources.firstIndex(where: { $0.id == resourceId }) {
                resources[index] = remote
            } else {
                resources.append(remote)
            }
            provisionalStaleAdditionIds.remove(resourceId)
            refreshDocumentTabs(for: resourceId)
        } else {
            staleResourceRefreshGenerations[snapshot.projectId] = UUID()
            resources.removeAll { $0.id == resourceId }
            provisionalStaleAdditionIds.remove(resourceId)
            if let tab = tabs.first(where: {
                $0.itemId == resourceId && $0.projectId == key.projectId
            }) {
                closeTab(tab)
            }
        }
        staleResourceSnapshots.removeValue(forKey: resourceId)
        refreshVisibleStaleResourceIds()
        bumpDocumentContentGeneration(for: resourceId)
        advanceProjectRefIfPlanCompleted(
            projectId: snapshot.projectId,
            authoritativeCommitId: snapshot.authoritativeCommitId,
            authoritativeRefEtag: snapshot.authoritativeRefEtag,
            selectedOrgResourceIds: snapshot.selectedOrgResourceIds,
            orgSelectionRevision: snapshot.orgSelectionRevision
        )
        endDocumentSynchronization(key, generation: generation)
    }

    /// Builds the three-way unified diff presentation for one document:
    /// local changes (base -> draft) render green/red, remote changes
    /// (base -> latest shared) render gray.
    func documentDiffPresentation(
        for item: MemoryListItem,
        localText: String
    ) async throws -> DocumentDiffResult? {
        if let draft = item.draft {
            if draft.freshness == .behind {
                let candidate = try await reconciliationCandidate(for: draft)
                return .init(
                    presentation: UnifiedDiffPresentation(lines: ThreeWayDiff.lines(
                        base: candidate.baseState.exists
                            ? (candidate.baseState.content?.content ?? "") : "",
                        local: candidate.draftState.exists
                            ? (candidate.draftState.content?.content ?? "") : "",
                        remote: candidate.currentState.exists
                            ? (candidate.currentState.content?.content ?? "") : ""
                    )),
                    pathChanges: Self.documentPathChanges(
                        basePath: candidate.baseState.exists
                            ? candidate.baseState.resource.path : nil,
                        localPath: candidate.draftState.exists
                            ? candidate.draftState.resource.path : nil,
                        remotePath: candidate.currentState.exists
                            ? candidate.currentState.resource.path : nil
                    )
                )
            }
            let sharedText = item.resource?.document.body ?? ""
            return .init(
                presentation: UnifiedDiffPresentation(lines: ThreeWayDiff.lines(
                    base: sharedText,
                    local: draft.isDeletion ? "" : localText,
                    remote: sharedText
                )),
                pathChanges: documentPathChanges(for: item)
            )
        }
        if item.resource != nil,
           let snapshot = staleResourceSnapshot(for: item) {
            let texts = try Self.staleDocumentDiffTexts(snapshot)
            return .init(
                presentation: UnifiedDiffPresentation(lines: ThreeWayDiff.lines(
                    base: texts.base,
                    local: texts.base,
                    remote: texts.remote
                )),
                pathChanges: documentPathChanges(for: item)
            )
        }
        return nil
    }

    nonisolated static func staleDocumentDiffTexts(
        _ snapshot: StaleResourceSyncSnapshot
    ) throws -> (base: String, remote: String) {
        if let local = snapshot.local, !local.contentLoaded {
            throw DocumentDiffError.baselineUnavailable
        }
        return (
            base: snapshot.local?.document.body ?? "",
            remote: snapshot.remote?.document.body ?? ""
        )
    }

    private func hydrateStaleResourcePlan(
        _ plan: [String: StaleResourceSyncSnapshot]
    ) async -> [String: StaleResourceSyncSnapshot] {
        var hydrated = plan
        var payloads: [String: CommitPayload] = [:]
        var unavailableCommitIds = Set<String>()

        for (resourceId, snapshot) in plan {
            guard var local = snapshot.local else { continue }
            // Some files may already have applied an earlier generation while
            // the Project ref intentionally remains at the all-files barrier.
            // Hydrate each file from its own generation first.
            let commitId = local.refCommitId ?? snapshot.observedProjectRefCommitId
            var historicalBody: String?
            if let commitId, !unavailableCommitIds.contains(commitId) {
                let payload: CommitPayload?
                if let cached = payloads[commitId] {
                    payload = cached
                } else {
                    do {
                        let loaded = try await loadCommit(commitId)
                        if let loaded {
                            payloads[commitId] = loaded
                        } else {
                            unavailableCommitIds.insert(commitId)
                        }
                        payload = loaded
                    } catch {
                        unavailableCommitIds.insert(commitId)
                        payload = nil
                    }
                }
                if let payload,
                   let entry = payload.tree.entries.first(where: { entry in
                    entry.type == .memory && entry.id == local.id
                   }),
                   let blob = payload.blobs.first(where: { $0.blobId == entry.blobId }),
                   Self.contentHash(blob.content) == local.contentHash {
                    historicalBody = blob.content
                }
            }

            if let historicalBody {
                local.document.body = historicalBody
                local.contentLoaded = true
            } else if Self.contentHash(local.document.body) == local.contentHash {
                // A loaded body is only an acceptable fallback when its hash
                // proves it belongs to the observed generation.
                local.contentLoaded = true
            } else {
                local.document.body = ""
                local.contentLoaded = false
            }
            hydrated[resourceId] = .init(
                projectId: snapshot.projectId,
                observedProjectRefCommitId: snapshot.observedProjectRefCommitId,
                observedSelectedOrgResourceIds: snapshot.observedSelectedOrgResourceIds,
                observedOrgSelectionRevision: snapshot.observedOrgSelectionRevision,
                authoritativeCommitId: snapshot.authoritativeCommitId,
                authoritativeRefEtag: snapshot.authoritativeRefEtag,
                selectedOrgResourceIds: snapshot.selectedOrgResourceIds,
                orgSelectionRevision: snapshot.orgSelectionRevision,
                generation: snapshot.generation,
                local: local,
                remote: snapshot.remote
            )
        }
        return hydrated
    }

    nonisolated private static func contentHash(_ content: String) -> String {
        let digest = SHA256.hash(data: Data(content.utf8))
        return "sha256:" + digest.map { String(format: "%02x", $0) }.joined()
    }

    private func installStaleResourcePlan(
        _ plan: [String: StaleResourceSyncSnapshot],
        for projectId: String
    ) {
        let applicablePlan = plan.filter { resourceId, snapshot in
            if let local = snapshot.local {
                guard let current = resources.first(where: { $0.id == resourceId }) else {
                    return false
                }
                return Self.resourceGenerationMatches(current, local)
            }
            return !resources.contains(where: { $0.id == resourceId })
                || provisionalStaleAdditionIds.contains(resourceId)
        }
        clearStaleResourceState(for: projectId)
        for (resourceId, snapshot) in applicablePlan {
            staleResourceSnapshots[resourceId] = snapshot
            if let local = snapshot.local,
               local.contentLoaded,
               let index = resources.firstIndex(where: { $0.id == resourceId }),
               resources[index] != local {
                resources[index] = local
                bumpDocumentContentGeneration(for: resourceId)
            }
            if snapshot.local == nil,
               let remote = snapshot.remote,
               !resources.contains(where: { $0.id == resourceId }) {
                resources.append(remote)
                provisionalStaleAdditionIds.insert(resourceId)
            }
        }
        refreshVisibleStaleResourceIds()
    }

    private func clearStaleResourceState(for projectId: String) {
        staleResourceRefreshGenerations[projectId] = UUID()
        let ids = Set(staleResourceSnapshots.compactMap { resourceId, snapshot in
            snapshot.projectId == projectId ? resourceId : nil
        })
        let provisionalIds = ids.intersection(provisionalStaleAdditionIds)
        if !provisionalIds.isEmpty {
            resources.removeAll { provisionalIds.contains($0.id) }
            provisionalStaleAdditionIds.subtract(provisionalIds)
            for resourceId in provisionalIds {
                bumpDocumentContentGeneration(for: resourceId)
            }
        }
        staleResourceSnapshots = staleResourceSnapshots.filter { _, snapshot in
            snapshot.projectId != projectId
        }
        refreshVisibleStaleResourceIds()
    }

    private func clearAllStaleResourceState() {
        if !provisionalStaleAdditionIds.isEmpty {
            resources.removeAll { provisionalStaleAdditionIds.contains($0.id) }
        }
        provisionalStaleAdditionIds.removeAll()
        staleResourceSnapshots.removeAll()
        staleResourceIds.removeAll()
        staleResourceRefreshGenerations.removeAll()
    }

    private func refreshVisibleStaleResourceIds() {
        guard let activeProjectId else {
            staleResourceIds.removeAll()
            return
        }
        staleResourceIds = Set(staleResourceSnapshots.compactMap { resourceId, snapshot in
            snapshot.projectId == activeProjectId ? resourceId : nil
        })
    }

    private func advanceProjectRefIfPlanCompleted(
        projectId: String,
        authoritativeCommitId: String,
        authoritativeRefEtag: String?,
        selectedOrgResourceIds: Set<String>,
        orgSelectionRevision: Int
    ) {
        guard !staleResourceSnapshots.values.contains(where: { $0.projectId == projectId }),
              let index = projects.firstIndex(where: { $0.id == projectId }) else {
            return
        }
        let current = projects[index]
        projects[index] = .init(
            id: current.id,
            name: current.name,
            refCommitId: authoritativeCommitId,
            refEtag: authoritativeRefEtag ?? current.refEtag,
            selectedOrgResourceIds: selectedOrgResourceIds,
            orgSelectionRevision: orgSelectionRevision,
            isLoaded: current.isLoaded
        )
        for resourceIndex in resources.indices
        where resources[resourceIndex].scope == .project
            && resources[resourceIndex].projectId == projectId
            && resources[resourceIndex].refCommitId != authoritativeCommitId {
            let resource = resources[resourceIndex]
            resources[resourceIndex] = .init(
                id: resource.id,
                scope: resource.scope,
                projectId: resource.projectId,
                projectName: resource.projectName,
                kind: resource.kind,
                contentHash: resource.contentHash,
                updatedAt: resource.updatedAt,
                refCommitId: authoritativeCommitId,
                contentLoaded: resource.contentLoaded,
                document: resource.document
            )
        }
    }

    private func bumpDocumentContentGeneration(for resourceId: String) {
        documentContentGenerations[resourceId, default: 0] &+= 1
    }

    private func replaceProjectResources(
        projectId: String,
        with replacements: [MemoryResource]
    ) {
        let previous = Dictionary(
            resources.lazy.filter { $0.projectId == projectId }.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        resources.removeAll { $0.projectId == projectId }
        resources += replacements
        let next = Dictionary(
            replacements.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        for resourceId in Set(previous.keys).union(next.keys)
        where previous[resourceId] != next[resourceId] {
            bumpDocumentContentGeneration(for: resourceId)
        }
        refreshAllDocumentTabs()
    }

    private func prepareBehindDraftSync(
        key: MemoryDocumentSessionKey,
        draft: LocalDraft,
        generation: UUID
    ) async {
        guard isCurrentDocumentSynchronization(key, generation: generation) else { return }
        do {
            // Keep the editor locked across the single upload barrier and the
            // candidate POST so a late keystroke cannot be omitted.
            let synchronized = try await synchronizedDraftForReconciliation(
                itemId: key.itemId,
                draft: draft
            )
            guard isCurrentDocumentSynchronization(key, generation: generation) else {
                return
            }
            installSynchronizedDraft(synchronized)
            guard synchronized.freshness == .behind else {
                // A provisional remote addition already uses the authoritative
                // generation as its draft base. It needs adoption, not a
                // reconciliation candidate for an already-current draft.
                if let resourceId = synchronized.targetId {
                    adoptCurrentStaleResource(resourceId)
                }
                endDocumentSynchronization(key, generation: generation)
                return
            }
            let candidate = try await requestReconciliationCandidate(for: synchronized)
            guard isCurrentDocumentSynchronization(key, generation: generation),
                  tabs.contains(where: {
                      $0.itemId == key.itemId && $0.projectId == key.projectId
                  }) else {
                endDocumentSynchronization(key, generation: generation)
                return
            }
            pendingDocumentReconciliationCandidatesBySession[key] = candidate
            documentReconciliationResolutions[key] = candidate.proposedState
                ?? candidate.draftState
        } catch is CancellationError {
            endDocumentSynchronization(key, generation: generation)
            return
        } catch {
            guard isCurrentDocumentSynchronization(key, generation: generation) else { return }
            endDocumentSynchronization(key, generation: generation)
            errorMessage = error.localizedDescription
        }
    }

    private func adoptCurrentStaleResource(_ resourceId: String) {
        guard let snapshot = staleResourceSnapshots.removeValue(forKey: resourceId) else { return }
        staleResourceRefreshGenerations[snapshot.projectId] = UUID()
        provisionalStaleAdditionIds.remove(resourceId)
        refreshVisibleStaleResourceIds()
        advanceProjectRefIfPlanCompleted(
            projectId: snapshot.projectId,
            authoritativeCommitId: snapshot.authoritativeCommitId,
            authoritativeRefEtag: snapshot.authoritativeRefEtag,
            selectedOrgResourceIds: snapshot.selectedOrgResourceIds,
            orgSelectionRevision: snapshot.orgSelectionRevision
        )
    }

    private func synchronizedDraftForReconciliation(
        itemId: String,
        draft: LocalDraft
    ) async throws -> LocalDraft {
        let sessionKey = MemoryDocumentSessionKey(
            projectId: draft.projectId,
            itemId: itemId
        )
        if pendingDocumentSaves[sessionKey] != nil {
            try await flushDocumentSave(sessionKey)
        }

        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(15))
        var requestedRetry = false
        var requiresPostRetryCheck = false
        while clock.now < deadline || requiresPostRetryCheck {
            requiresPostRetryCheck = false
            try Task.checkCancellation()
            let detail = try await daemon.draft(draft.id)
            let failure = detail.operations.reversed().first {
                $0.syncStatus == .failed
            }?.lastError
            switch Self.draftUploadBarrierDecision(
                serverDraftId: detail.draft.serverDraftId,
                pendingOperationCount: detail.draft.pendingOperationCount,
                failedOperationCount: detail.draft.failedOperationCount,
                operationStates: detail.operations.map(\.syncStatus),
                failureMessage: failure
            ) {
            case .failed(let message):
                throw DocumentSyncError.draftUploadFailed(message)
            case .wait:
                if !requestedRetry {
                    let outcome = await retrySync(
                        channel: "drafts",
                        projectId: draft.projectId
                    )
                    if case .failed(let message) = outcome {
                        throw DocumentSyncError.draftUploadFailed(message)
                    }
                    requestedRetry = true
                    requiresPostRetryCheck = true
                    continue
                }
            case .ready:
                // A user edit may have been staged while the daemon was
                // uploading. Flush it and repeat the barrier before creating
                // a candidate with an authoritative serverVersion.
                if pendingDocumentSaves[sessionKey] != nil {
                    try await flushDocumentSave(sessionKey)
                    requestedRetry = false
                    requiresPostRetryCheck = true
                    continue
                }
                let mapped = WorkspaceLoader.mapDraft(detail, resources: resources)
                return mapped
            }
            try await Task.sleep(for: .milliseconds(150))
        }
        throw DocumentSyncError.draftUploadTimedOut
    }

    private func installSynchronizedDraft(_ draft: LocalDraft) {
        if let index = drafts.firstIndex(where: { $0.id == draft.id }) {
            guard drafts[index].serverVersion <= draft.serverVersion else { return }
            drafts[index] = draft
        } else {
            drafts.append(draft)
        }
    }

    func addOrgMemories(resourceIds: Set<String>, toProject projectId: String) async throws {
        try await mutateProjectOrgSelection(
            projectId: projectId,
            resourceIds: resourceIds,
            mutation: .add
        )
    }

    func removeOrgMemories(
        resourceIds: Set<String>,
        fromProject projectId: String
    ) async throws {
        try await mutateProjectOrgSelection(
            projectId: projectId,
            resourceIds: resourceIds,
            mutation: .remove
        )
    }

    private func mutateProjectOrgSelection(
        projectId: String,
        resourceIds: Set<String>,
        mutation: ProjectOrgSelectionMutation
    ) async throws {
        guard canManageProject(projectId) else {
            throw ServerClientError.forbidden("Project administrator access is required to manage project memory.")
        }
        guard !hasDocumentSynchronization(in: projectId) else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        guard projects.contains(where: { $0.id == projectId }) else {
            throw ProjectMemorySelectionError.projectUnavailable
        }
        try validateOrgResourceIds(resourceIds)
        try await withProjectOrgSelectionMutation {
            guard canManageProject(projectId) else {
                throw ServerClientError.forbidden(
                    "Project administrator access is required to manage project memory."
                )
            }
            guard projects.contains(where: { $0.id == projectId }) else {
                throw ProjectMemorySelectionError.projectUnavailable
            }
            try validateOrgResourceIds(resourceIds)
            guard !hasDocumentSynchronization(in: projectId),
                  projectOrgSelectionMutatingIds.insert(projectId).inserted else {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            defer { projectOrgSelectionMutatingIds.remove(projectId) }
            // Removing a selected Org resource can make its Project tab and
            // tree row disappear. Materialize every dirty editor in that
            // Project first so a failed save remains visible and recoverable
            // as a LocalDraft instead of being stranded in a debounce buffer.
            let pendingSaveKeys = pendingDocumentSaves.compactMap { key, pending in
                let pendingIds = Set(
                    [pending.item.id, pending.item.draft?.id, pending.item.draft?.targetId]
                        .compactMap { $0 }
                )
                return key.projectId == projectId && !pendingIds.isDisjoint(with: resourceIds)
                    ? key
                    : nil
            }
            for key in pendingSaveKeys {
                try await flushDocumentSave(key)
            }
            if case .remove = mutation,
               Self.hasActiveDraft(
                   in: projectId,
                   targetingAny: resourceIds,
                   drafts: drafts
               ) {
                throw ProjectMemorySelectionError.activeDrafts
            }
            let current: ProjectOrgSelection = try await server.get(
                "/api/v1/projects/\(projectId)/org-selections"
            )
            let currentIds = projectOrgResourceIds(current)
            let nextIds = mutation.applying(resourceIds, to: currentIds)
            guard nextIds != currentIds else {
                // The Server may already reflect the requested state while a
                // stale local snapshot does not. Treat the authoritative GET
                // as a successful repair instead of leaving the UI behind.
                await applyProjectOrgSelection(current, toProject: projectId)
                return
            }
            guard canManageProject(projectId) else {
                throw ServerClientError.forbidden(
                    "Project administrator access is required to manage project memory."
                )
            }
            guard projects.contains(where: { $0.id == projectId }) else {
                throw ProjectMemorySelectionError.projectUnavailable
            }
            guard !hasDocumentSynchronization(in: projectId) else {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            let selection = try await replaceProjectOrgSelection(
                projectId: projectId,
                expectedRevision: current.revision,
                resourceIds: nextIds
            )
            await applyProjectOrgSelection(selection, toProject: projectId)
        }
    }

    private func applyProjectOrgSelection(
        _ selection: ProjectOrgSelection,
        toProject projectId: String
    ) async {
        let commit: (value: CommitStateResponse, response: DaemonServerResponse)? = try? await server.getWithMetadata(
            "/api/v1/projects/\(projectId)/commit-state"
        )
        let freshCommit = commit.flatMap { $0.response.isStaleCache ? nil : $0 }
        guard let index = projects.firstIndex(where: { $0.id == projectId }) else { return }
        let project = projects[index]
        guard selection.revision >= project.orgSelectionRevision else { return }
        clearStaleResourceState(for: projectId)
        projects[index] = ProjectState(
            id: project.id,
            name: project.name,
            refCommitId: freshCommit?.value.ref.commitId ?? project.refCommitId,
            refEtag: freshCommit?.response.headers.first {
                $0.key.caseInsensitiveCompare("etag") == .orderedSame
            }?.value ?? project.refEtag,
            selectedOrgResourceIds: projectOrgResourceIds(selection),
            orgSelectionRevision: selection.revision,
            isLoaded: project.isLoaded
        )
        pruneOrphanedMemoryTabs()
        if projectId == activeProjectId {
            _ = await retrySync(channel: "commits", projectId: projectId)
        }
    }

    private func replaceProjectOrgSelection(
        projectId: String,
        expectedRevision: Int,
        resourceIds: Set<String>
    ) async throws -> ProjectOrgSelection {
        let request = ReplaceProjectOrgSelectionRequest(resourceIds: resourceIds.sorted())
        return try await server.send(
            method: "PUT",
            path: "/api/v1/projects/\(projectId)/org-selections",
            headers: ["If-Match": String(expectedRevision)],
            body: request
        )
    }

    private func initializeProjectMemory(
        projectId: String,
        bundleId: String?,
        generation: UUID
    ) async throws -> ProjectOrgSelection {
        let current: ProjectOrgSelection = try await server.get(
            "/api/v1/projects/\(projectId)/org-selections"
        )
        try ensureCurrentAdministrationMutation(generation)
        guard let bundleId else { return current }
        guard let bundle = bundles.first(where: { $0.id == bundleId }) else {
            throw ProjectSetupError.bundleNotFound
        }
        let resourceIds = Set(bundle.resourceIds)
        do {
            try validateOrgResourceIds(resourceIds)
        } catch {
            throw ProjectSetupError.bundleContainsUnavailableMemory
        }
        guard projectOrgResourceIds(current) != resourceIds else { return current }
        return try await replaceProjectOrgSelection(
            projectId: projectId,
            expectedRevision: current.revision,
            resourceIds: resourceIds
        )
    }

    private func validateOrgResourceIds(_ resourceIds: Set<String>) throws {
        let available = Set(resources.lazy.filter { $0.scope == .org }.map(\.id))
        guard resourceIds.isSubset(of: available) else {
            throw ProjectMemorySelectionError.invalidOrgResources
        }
    }

    private func projectOrgResourceIds(_ selection: ProjectOrgSelection) -> Set<String> {
        Set(selection.memories.map(\.memoryId))
    }

    func createBundle() async {
        do {
            try await withBundleMutation {
                let detail: PersonalBundleDetail = try await server.send(
                    method: "POST",
                    path: "/api/v1/me/bundles",
                    body: PersonalBundleRequest(
                        name: "Untitled Bundle",
                        description: "",
                        resourceIds: []
                    )
                )
                let bundle = PersonalBundle(
                    id: detail.bundle.bundleId,
                    name: detail.bundle.name,
                    description: detail.bundle.description,
                    resourceIds: [],
                    revision: detail.bundle.revision,
                    updatedAt: detail.bundle.updatedAt
                )
                bundles.insert(bundle, at: 0)
                selectedBundleId = bundle.id
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func updateBundle(
        _ bundle: PersonalBundle,
        name: String,
        description: String,
        resourceIds: Set<String>
    ) async throws {
        try await withBundleMutation {
            guard let current = bundles.first(where: { $0.id == bundle.id }) else { return }
            try validateOrgResourceIds(resourceIds)
            let selected = resources.filter {
                $0.scope == .org && resourceIds.contains($0.id)
            }
            let request = PersonalBundleRequest(
                name: name,
                description: description,
                resourceIds: selected.map(\.id)
            )
            let detail: PersonalBundleDetail = try await server.send(
                method: "PATCH",
                path: "/api/v1/me/bundles/\(bundle.id)",
                headers: ["If-Match": String(current.revision)],
                body: request
            )
            let updated = PersonalBundle(
                id: detail.bundle.bundleId,
                name: detail.bundle.name,
                description: detail.bundle.description,
                resourceIds: detail.memories.map(\.memoryId),
                revision: detail.bundle.revision,
                updatedAt: detail.bundle.updatedAt
            )
            if let index = bundles.firstIndex(where: { $0.id == updated.id }) {
                bundles[index] = updated
            }
        }
    }

    func deleteBundle(_ bundle: PersonalBundle) async {
        do {
            try await withBundleMutation {
                guard let current = bundles.first(where: { $0.id == bundle.id }) else { return }
                let response = try await server.raw(
                    method: "DELETE",
                    path: "/api/v1/me/bundles/\(bundle.id)",
                    headers: ["If-Match": String(current.revision)]
                )
                guard (200..<300).contains(response.status) else {
                    throw ServerClientError.response(status: response.status, message: response.body)
                }
                bundles.removeAll { $0.id == bundle.id }
                selectedBundleId = bundles.first?.id
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func requestReview(
        for draft: LocalDraft,
        title: String,
        description: String,
        candidate: DraftReconciliationCandidate? = nil,
        resolvedState: ReconciliationResourceState? = nil
    ) async throws {
        if synchronizationItemId(for: draft) != nil {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        guard Self.canRequestReview(draft) else {
            throw ReviewRequestError.legacyProjectDraftCannotBePublished
        }
        guard let serverId = draft.serverId else {
            throw ReviewRequestError.draftNotSynchronized
        }
        guard draft.freshness == .current || candidate != nil else {
            throw ReviewRequestError.reconciliationRequired
        }
        let detail: ReviewDetail = try await server.send(
            method: "POST",
            path: "/api/v1/reviews",
            headers: ["If-Match": Self.refETag(candidate?.currentCommitId ?? draft.currentCommitId)],
            body: CreateReviewRequest(
                drafts: [ReviewDraftRequest(
                    draftId: serverId,
                    expectedDraftVersion: candidate?.draftVersion ?? draft.serverVersion,
                    candidateId: candidate?.candidateId,
                    resolvedState: resolvedState
                )],
                title: title,
                description: description
            )
        )
        let record = WorkspaceLoader.mapReview(detail)
        reviews.insert(record, at: 0)
        selectedReviewId = record.id
        selectedSection = .reviews
    }

    func requestReview(
        for drafts: [LocalDraft],
        title: String,
        description: String,
        reconciliations: [ReviewDraftReconciliation] = []
    ) async throws {
        let selectedDrafts = drafts.sorted {
            $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending
        }
        guard let selectedPrimary = selectedDrafts.first else { return }
        guard selectedDrafts.allSatisfy({ $0.projectId == selectedPrimary.projectId }) else {
            throw ReviewRequestError.mixedProjects
        }
        guard selectedDrafts.allSatisfy(Self.canRequestReview) else {
            throw ReviewRequestError.legacyProjectDraftCannotBePublished
        }
        guard selectedDrafts.allSatisfy({ synchronizationItemId(for: $0) == nil }) else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        var drafts = [LocalDraft]()
        drafts.reserveCapacity(selectedDrafts.count)
        for draft in selectedDrafts {
            drafts.append(
                try await synchronizedDraftForReconciliation(
                    itemId: draft.targetId ?? draft.id,
                    draft: draft
                )
            )
        }
        guard let primary = drafts.first else { return }
        guard drafts.allSatisfy({ $0.serverId != nil }) else {
            throw ReviewRequestError.draftNotSynchronized
        }
        let reconciliationByDraftId = Dictionary(
            reconciliations.map { ($0.candidate.draftId, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        guard drafts.allSatisfy({ draft in
            draft.freshness == .current
                || draft.serverId.flatMap { reconciliationByDraftId[$0] } != nil
        }) else {
            throw ReviewRequestError.reconciliationRequired
        }
        let selectedDraftIds = Set(drafts.compactMap(\.serverId))
        guard reconciliationByDraftId.keys.allSatisfy(selectedDraftIds.contains) else {
            throw ReviewRequestError.reconciliationRequired
        }
        let detail: ReviewDetail = try await server.send(
            method: "POST",
            path: "/api/v1/reviews",
            headers: [
                "If-Match": Self.refETag(
                    reconciliations.first?.candidate.currentCommitId ?? primary.currentCommitId
                )
            ],
            body: CreateReviewRequest(
                drafts: drafts.map { draft in
                    let reconciliation = reconciliationByDraftId[draft.serverId!]
                    return ReviewDraftRequest(
                        draftId: draft.serverId!,
                        expectedDraftVersion: reconciliation?.candidate.draftVersion
                            ?? draft.serverVersion,
                        candidateId: reconciliation?.candidate.candidateId,
                        resolvedState: reconciliation?.resolvedState
                    )
                },
                title: title,
                description: description
            )
        )
        let record = WorkspaceLoader.mapReview(detail)
        reviews.insert(record, at: 0)
        selectedReviewId = record.id
        selectedSection = .reviews
    }

    func resubmit(
        _ review: ReviewRecord,
        detail: ReviewDetail,
        candidate: DraftReconciliationCandidate? = nil,
        resolvedState: ReconciliationResourceState? = nil
    ) async throws {
        guard Self.isOrganizationDraft(detail.draft) else {
            throw ReviewRequestError.legacyProjectDraftCannotBePublished
        }
        guard isReviewAuthor(review) else {
            throw ServerClientError.forbidden("Only the draft author can resubmit this Review.")
        }
        guard detail.draft.coordination.freshness == .current || candidate != nil else {
            throw ReviewRequestError.reconciliationRequired
        }
        let updated: ReviewDetail = try await server.send(
            method: "POST",
            path: "/api/v1/reviews/\(review.id)/submissions",
            headers: [
                "If-Match": Self.refETag(
                    candidate?.currentCommitId ?? detail.draft.coordination.currentCommitId
                )
            ],
            body: CreateReviewSubmissionRequest(
                expectedReviewVersion: review.version,
                drafts: (detail.drafts ?? [
                    ReviewDraftDetail(draft: detail.draft, operations: detail.operations)
                ]).map { draftDetail in
                    let reconciliation = candidate.flatMap { candidate in
                        candidate.draftId == draftDetail.draft.draftId ? candidate : nil
                    }
                    return ReviewDraftRequest(
                        draftId: draftDetail.draft.draftId,
                        expectedDraftVersion: reconciliation?.draftVersion
                            ?? draftDetail.draft.version,
                        candidateId: reconciliation?.candidateId,
                        resolvedState: reconciliation == nil ? nil : resolvedState
                    )
                },
                title: review.title,
                description: review.description
            )
        )
        replaceReview(with: WorkspaceLoader.mapReview(updated))
    }

    func reconciliationCandidate(for draft: LocalDraft) async throws -> DraftReconciliationCandidate {
        let itemId = draft.targetId ?? draft.id
        let key = MemoryDocumentSessionKey(projectId: draft.projectId, itemId: itemId)
        guard !isSwitchingMemoryContext,
              activeProjectId == draft.projectId,
              synchronizingDocumentSessions.insert(key).inserted else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        let generation = UUID()
        documentSynchronizationGenerations[key] = generation
        defer { endDocumentSynchronization(key, generation: generation) }

        let synchronized = try await synchronizedDraftForReconciliation(
            itemId: itemId,
            draft: draft
        )
        try Task.checkCancellation()
        guard isCurrentDocumentSynchronization(key, generation: generation) else {
            throw CancellationError()
        }
        let candidate = try await requestReconciliationCandidate(for: synchronized)
        try Task.checkCancellation()
        guard isCurrentDocumentSynchronization(key, generation: generation) else {
            throw CancellationError()
        }
        return candidate
    }

    func reconciliationCandidates(
        for drafts: [LocalDraft]
    ) async throws -> [DraftReconciliationCandidate] {
        guard !isSwitchingMemoryContext else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        let selectedDrafts = drafts.sorted {
            $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending
        }
        var candidates = [DraftReconciliationCandidate]()
        for draft in selectedDrafts {
            let synchronized = try await synchronizedDraftForReconciliation(
                itemId: draft.targetId ?? draft.id,
                draft: draft
            )
            if synchronized.freshness == .behind {
                candidates.append(try await requestReconciliationCandidate(for: synchronized))
            }
        }
        return candidates
    }

    private func requestReconciliationCandidate(
        for synchronized: LocalDraft
    ) async throws -> DraftReconciliationCandidate {
        guard let serverId = synchronized.serverId else {
            throw ReviewRequestError.draftNotSynchronized
        }
        return try await server.send(
            method: "POST",
            path: "/api/v1/drafts/\(serverId)/reconciliation-candidates",
            body: CreateDraftReconciliationCandidateRequest(
                expectedDraftVersion: synchronized.serverVersion
            )
        )
    }

    func reconciliationCandidate(for detail: ReviewDetail) async throws -> DraftReconciliationCandidate {
        try await reconciliationCandidate(
            for: ReviewDraftDetail(draft: detail.draft, operations: detail.operations)
        )
    }

    func reconciliationCandidate(
        for detail: ReviewDraftDetail
    ) async throws -> DraftReconciliationCandidate {
        guard !isSwitchingMemoryContext else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        let activityId = UUID()
        standaloneReconciliationActivityIds.insert(activityId)
        defer { standaloneReconciliationActivityIds.remove(activityId) }
        return try await server.send(
            method: "POST",
            path: "/api/v1/drafts/\(detail.draft.draftId)/reconciliation-candidates",
            body: CreateDraftReconciliationCandidateRequest(
                expectedDraftVersion: detail.draft.version
            )
        )
    }

    func applyReconciliation(
        draftId: String,
        candidate: DraftReconciliationCandidate,
        resolvedState: ReconciliationResourceState?,
        projectId: String? = nil,
        documentItemId: String? = nil
    ) async throws {
        guard !isSwitchingMemoryContext else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        var documentKey: MemoryDocumentSessionKey?
        var standaloneActivityId: UUID?
        if let documentItemId {
            guard let key = activeDocumentSessionKey(for: documentItemId),
                  synchronizingDocumentSessions.contains(key),
                  pendingDocumentReconciliationCandidatesBySession[key]?.candidateId
                    == candidate.candidateId,
                  applyingDocumentReconciliationSessions.insert(key).inserted else {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            documentKey = key
        } else {
            let activityId = UUID()
            standaloneReconciliationActivityIds.insert(activityId)
            standaloneActivityId = activityId
        }
        defer {
            if let documentKey {
                applyingDocumentReconciliationSessions.remove(documentKey)
            }
            if let standaloneActivityId {
                standaloneReconciliationActivityIds.remove(standaloneActivityId)
            }
        }
        guard let reconciliationProjectId = projectId ?? documentKey?.projectId else {
            throw ProjectMemorySelectionError.projectUnavailable
        }
        let _: DraftRebaseResult = try await server.send(
            method: "POST",
            path: "/api/v1/drafts/\(draftId)/rebases",
            headers: ["If-Match": Self.refETag(candidate.currentCommitId)],
            body: CreateDraftRebaseRequest(
                candidateId: candidate.candidateId,
                expectedDraftVersion: candidate.draftVersion,
                resolvedState: resolvedState
            )
        )
        _ = await retrySync(channel: "drafts", projectId: reconciliationProjectId)
        await reload(allowsDuringDocumentReconciliation: true)
    }

    func reviewDetail(_ reviewId: String) async throws -> ReviewDetail {
        try await server.get("/api/v1/reviews/\(reviewId)")
    }

    func addComment(
        _ body: String,
        to review: ReviewRecord,
        anchorPath: String? = nil,
        anchorLine: Int? = nil
    ) async throws {
        let _: ReviewComment = try await server.send(
            method: "POST",
            path: "/api/v1/reviews/\(review.id)/comments",
            body: CreateReviewCommentRequest(
                body: body,
                expectedReviewVersion: review.version,
                anchorPath: anchorPath,
                anchorLine: anchorLine
            )
        )
        try await refreshReview(review.id)
    }

    func decide(_ review: ReviewRecord, decision: String, note: String) async throws {
        let detail: ReviewDetail = try await server.send(
            method: "POST",
            path: "/api/v1/reviews/\(review.id)/decisions",
            body: CreateReviewDecisionRequest(
                decision: decision,
                expectedReviewVersion: review.version,
                body: note
            )
        )
        replaceReview(with: WorkspaceLoader.mapReview(detail))
    }

    func merge(_ review: ReviewRecord) async throws {
        guard canMergeReviews else {
            throw ServerClientError.forbidden("Your account cannot merge Reviews.")
        }
        // A Review belongs to its carrying Project, while its Draft targets
        // Organization authority. The coordination commit is the exact Org Ref
        // generation; the carrying Project's projection ETag is not that base.
        let detail = try await reviewDetail(review.id)
        let currentReview = WorkspaceLoader.mapReview(detail)
        replaceReview(with: currentReview)
        guard ReviewDecisionReadiness(review: review).matches(currentReview) else {
            throw ReviewRequestError.reviewChanged
        }
        guard Self.isOrganizationDraft(detail.draft) else {
            throw ReviewRequestError.legacyProjectDraftCannotBePublished
        }
        let _: ReviewMergeResponse = try await server.send(
            method: "POST",
            path: "/api/v1/reviews/\(review.id)/merges",
            headers: ["If-Match": Self.refETag(detail.draft.coordination.currentCommitId)],
            body: CreateReviewMergeRequest(expectedReviewVersion: review.version)
        )
        await reload()
    }

    nonisolated static func canRequestReview(_ draft: LocalDraft) -> Bool {
        draft.scope == .org
    }

    nonisolated static func reviewableProjectDrafts(
        _ drafts: [LocalDraft],
        projectId: String?
    ) -> [LocalDraft] {
        preferredMemoryTreeDrafts(
            memoryTreeDrafts(drafts, activeProjectId: projectId)
        ).filter {
            $0.status == .open && canRequestReview($0)
        }
    }

    private nonisolated static func isOrganizationDraft(_ draft: ServerDraft) -> Bool {
        draft.resource.scope == MemoryScope.org.rawValue
    }

    private func currentDraft(for item: MemoryListItem) -> LocalDraft? {
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
            ?? activeProjectId
    }

    private func withDraftMutation<T>(_ operation: () async throws -> T) async throws -> T {
        await draftMutationGate.lock()
        do {
            try Task.checkCancellation()
            let result = try await operation()
            await draftMutationGate.unlock()
            return result
        } catch {
            await draftMutationGate.unlock()
            throw error
        }
    }

    private func withBundleMutation<T>(_ operation: () async throws -> T) async throws -> T {
        await bundleMutationGate.lock()
        do {
            try Task.checkCancellation()
            let result = try await operation()
            await bundleMutationGate.unlock()
            return result
        } catch {
            await bundleMutationGate.unlock()
            throw error
        }
    }

    private func withProjectOrgSelectionMutation<T>(
        _ operation: () async throws -> T
    ) async throws -> T {
        await projectOrgSelectionMutationGate.lock()
        do {
            try Task.checkCancellation()
            let result = try await operation()
            await projectOrgSelectionMutationGate.unlock()
            return result
        } catch {
            await projectOrgSelectionMutationGate.unlock()
            throw error
        }
    }

    private func persistDocumentSave(
        _ key: MemoryDocumentSessionKey,
        generation: UUID
    ) async {
        guard let pending = pendingDocumentSaves[key], pending.generation == generation else { return }
        guard synchronizationItemId(for: pending.item) == nil else {
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
                errorMessage = error.localizedDescription
            }
        }
    }

    private func persistBundleSave(_ bundleId: String, generation: UUID) async {
        guard let pending = pendingBundleSaves[bundleId], pending.generation == generation else { return }
        do {
            try await updateBundle(
                pending.bundle,
                name: pending.name,
                description: pending.description,
                resourceIds: pending.resourceIds
            )
            if pendingBundleSaves[bundleId]?.generation == generation {
                pendingBundleSaves[bundleId] = nil
                bundleSaveTasks[bundleId] = nil
            }
        } catch is CancellationError {
            return
        } catch {
            if pendingBundleSaves[bundleId]?.generation == generation {
                bundleSaveTasks[bundleId] = nil
                errorMessage = error.localizedDescription
            }
        }
    }

    private func invalidateAdministrationAuthority() {
        administrationMutationGeneration = UUID()
        isMutatingAdministration = false
        authorityGeneration = UUID()
    }

    func clearAuthorityScopedWorkspace() {
        cancelSyncRetries()
        resetBackgroundErrorPresentation()
        Self.invalidateWorkspaceTransitionState(
            generation: &projectSelectionGeneration,
            loadingProjectId: &loadingProjectId,
            isSwitchingMemoryContext: &isSwitchingMemoryContext,
            isPreparingWorkspaceIndex: &isPreparingWorkspaceIndex,
            orgResourceRefreshGeneration: &orgResourceRefreshGeneration
        )
        documentSaveTasks.values.forEach { $0.cancel() }
        documentSaveTasks.removeAll()
        pendingDocumentSaves.removeAll()
        bundleSaveTasks.values.forEach { $0.cancel() }
        bundleSaveTasks.removeAll()
        pendingBundleSaves.removeAll()
        account = nil
        organization = nil
        capabilities.removeAll()
        projectRoles.removeAll()
        projects.removeAll()
        invalidateAdministrationAuthority()
        orgRefCommitId = nil
        orgRefEtag = ""
        activeProjectId = nil
        resources.removeAll()
        drafts.removeAll()
        bundles.removeAll()
        reviews.removeAll()
        draftInventoryLoadState = .loading
        bundleLoadState = .loading
        reviewLoadState = .loading
        runtime = nil
        syncStatusAvailable = false
        selectedSection = .memory
        selectedItemId = nil
        selectedBundleId = nil
        selectedReviewId = nil
        pendingReviewReconciliationId = nil
        reviewDecisionReadiness = nil
        projectOrgSelectionMutatingIds.removeAll()
        applyingDocumentReconciliationSessions.removeAll()
        standaloneReconciliationActivityIds.removeAll()
        tabs.removeAll()
        activeTabId = nil
        navigationBackStack.removeAll()
        navigationForwardStack.removeAll()
        showsProjectSettings = false
        clearAllStaleResourceState()
        resourceLoadRequests.values.forEach { $0.task.cancel() }
        resourceLoadRequests.removeAll()
        loadingResourceIds.removeAll()
        documentSynchronizationTasks.values.forEach { $0.cancel() }
        documentSynchronizationTasks.removeAll()
        pendingDocumentReconciliationCandidatesBySession.removeAll()
        documentReconciliationResolutions.removeAll()
        pendingDocumentCommand = nil
        documentReconciliationToolbarState = nil
        synchronizingDocumentSessions.removeAll()
        documentSynchronizationGenerations.removeAll()
    }

    func apply(_ snapshot: WorkspaceSnapshot) {
        let preservesDeferredAuthority = Self.preservesDeferredAuthority(
            currentAccount: account,
            currentOrganization: organization,
            nextAccount: snapshot.account,
            nextOrganization: snapshot.organization
        )
        let previousResources = Dictionary(
            resources.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        if !preservesDeferredAuthority {
            clearAuthorityScopedWorkspace()
        } else {
            Self.invalidateWorkspaceTransitionState(
                generation: &projectSelectionGeneration,
                loadingProjectId: &loadingProjectId,
                isSwitchingMemoryContext: &isSwitchingMemoryContext,
                isPreparingWorkspaceIndex: &isPreparingWorkspaceIndex,
                orgResourceRefreshGeneration: &orgResourceRefreshGeneration
            )
        }
        clearAllStaleResourceState()
        resourceLoadRequests.values.forEach { $0.task.cancel() }
        resourceLoadRequests.removeAll()
        loadingResourceIds.removeAll()
        documentSynchronizationTasks.values.forEach { $0.cancel() }
        documentSynchronizationTasks.removeAll()
        pendingDocumentReconciliationCandidatesBySession.removeAll()
        documentReconciliationResolutions.removeAll()
        pendingDocumentCommand = nil
        documentReconciliationToolbarState = nil
        synchronizingDocumentSessions.removeAll()
        documentSynchronizationGenerations.removeAll()
        account = snapshot.account
        organization = snapshot.organization
        let lostOrganizationAuthority = canAdministerOrganization && !snapshot.capabilities.contains("admin:write")
        capabilities = snapshot.capabilities
        if lostOrganizationAuthority { invalidateAdministrationAuthority() }
        projects = snapshot.projects
        projectRoles = snapshot.projectRoles
        let accessibleProjectIds = Set(projects.map(\.id))
        drafts = Self.retainingAccessibleProjectRecords(
            drafts,
            accessibleProjectIds: accessibleProjectIds,
            projectId: \.projectId
        )
        reviews = Self.retainingAccessibleProjectRecords(
            reviews,
            accessibleProjectIds: accessibleProjectIds,
            projectId: \.projectId
        )
        if let selectedReviewId,
           !reviews.contains(where: { $0.id == selectedReviewId }) {
            self.selectedReviewId = nil
            reviewDecisionReadiness = nil
            pendingReviewReconciliationId = nil
        }
        orgRefCommitId = snapshot.orgRefCommitId
        orgRefEtag = snapshot.orgRefEtag
        activeProjectId = snapshot.activeProjectId
        resources = snapshot.resources
        let nextResources = Dictionary(
            resources.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        for resourceId in Set(previousResources.keys).union(nextResources.keys)
        where previousResources[resourceId] != nextResources[resourceId] {
            bumpDocumentContentGeneration(for: resourceId)
        }
        pruneOrphanedMemoryTabs()
        refreshAllDocumentTabs()
        runtime = snapshot.runtime
        applyLocalAgentAdapterResult(.init(
            conflicts: snapshot.legacyAgentAdapterConflicts,
            inspectionWarning: snapshot.legacyAgentAdapterInspectionWarning
        ))
        syncStatusAvailable = false
        if projects.isEmpty {
            selectedSection = .memory
            showsProjectSettings = false
            tabs = []
            activeTabId = nil
            selectedItemId = nil
        }
    }

    private func cancelPostReadyWork() {
        draftInventoryLoadTask?.cancel()
        draftInventoryLoadTask = nil
        bundleLoadTask?.cancel()
        bundleLoadTask = nil
        reviewLoadTask?.cancel()
        reviewLoadTask = nil
        legacyAgentAdapterInspectionTask?.cancel()
        legacyAgentAdapterInspectionTask = nil
        postReadyRetrySyncTask?.cancel()
        postReadyRetrySyncTask = nil
    }

    private func startPostReadyWork(
        generation: UUID,
        requiresFreshData: Bool,
        baseSnapshotWasStale: Bool
    ) {
        guard workspaceReloadGeneration == generation, phase == .ready else { return }
        let loader = WorkspaceLoader(daemon: daemon, bootstrap: bootstrap, server: server)
        let resourcesAtReady = resources
        let accessibleProjectIds = Set(projects.map(\.id))
        let baselineDrafts = drafts
        let baselineBundles = bundles
        let baselineReviews = reviews
        draftInventoryLoadState = .loading
        bundleLoadState = .loading
        reviewLoadState = .loading

        legacyAgentAdapterInspectionTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.workspaceReloadGeneration == generation {
                    self.legacyAgentAdapterInspectionTask = nil
                }
            }
            let result = await loader.inspectLegacyAgentAdapters()
            guard let self,
                  self.workspaceReloadGeneration == generation,
                  self.phase == .ready,
                  !Task.isCancelled else {
                return
            }
            self.applyLocalAgentAdapterResult(.init(
                conflicts: result.conflicts,
                inspectionWarning: Self.combinedAgentAdapterWarning(
                    self.legacyAgentAdapterInspectionWarning,
                    result.inspectionWarning
                )
            ))
        }

        draftInventoryLoadTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.workspaceReloadGeneration == generation {
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
                      self.workspaceReloadGeneration == generation,
                      self.phase == .ready else {
                    return
                }
                guard Self.canPublishDeferredLoad(
                    requiresFreshData: requiresFreshData,
                    baseSnapshotWasStale: baseSnapshotWasStale,
                    responseWasStale: loaded.hasStaleServerResponse
                ) else {
                    self.draftInventoryLoadState = .failed(
                        "Fresh Draft data was unavailable. Existing Drafts were kept."
                    )
                    return
                }
                for resource in loaded.loadedBaselines {
                    self.installLoadedResourceIfCurrent(resource)
                }
                self.drafts = Self.mergeDeferredRecords(
                    baseline: baselineDrafts,
                    current: self.drafts,
                    loaded: loaded.drafts
                )
                self.draftInventoryLoadState = .loaded
                self.pruneOrphanedMemoryTabs()
                self.refreshAllDocumentTabs()
            } catch is CancellationError {
                return
            } catch {
                guard let self, self.workspaceReloadGeneration == generation else { return }
                self.draftInventoryLoadState = .failed(error.localizedDescription)
            }
        }

        bundleLoadTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.workspaceReloadGeneration == generation {
                    self.bundleLoadTask = nil
                }
            }
            do {
                let loaded = try await loader.loadBundles()
                try Task.checkCancellation()
                guard let self,
                      self.workspaceReloadGeneration == generation,
                      self.phase == .ready else {
                    return
                }
                guard Self.canPublishDeferredLoad(
                    requiresFreshData: requiresFreshData,
                    baseSnapshotWasStale: baseSnapshotWasStale,
                    responseWasStale: loaded.hasStaleServerResponse
                ) else {
                    self.bundleLoadState = .failed(
                        "Fresh Bundle data was unavailable. Existing Bundles were kept."
                    )
                    return
                }
                self.bundles = Self.mergeDeferredRecords(
                    baseline: baselineBundles,
                    current: self.bundles,
                    loaded: loaded.records
                )
                self.bundleLoadState = .loaded
                if let selectedBundleId = self.selectedBundleId,
                   !self.bundles.contains(where: { $0.id == selectedBundleId }) {
                    self.selectedBundleId = self.bundles.first?.id
                } else {
                    self.selectedBundleId = self.selectedBundleId ?? self.bundles.first?.id
                }
            } catch is CancellationError {
                return
            } catch {
                guard let self, self.workspaceReloadGeneration == generation else { return }
                self.bundleLoadState = .failed(error.localizedDescription)
            }
        }

        reviewLoadTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.workspaceReloadGeneration == generation {
                    self.reviewLoadTask = nil
                }
            }
            do {
                let loaded = try await loader.loadReviews()
                try Task.checkCancellation()
                guard let self,
                      self.workspaceReloadGeneration == generation,
                      self.phase == .ready else {
                    return
                }
                guard Self.canPublishDeferredLoad(
                    requiresFreshData: requiresFreshData,
                    baseSnapshotWasStale: baseSnapshotWasStale,
                    responseWasStale: loaded.hasStaleServerResponse
                ) else {
                    self.reviewLoadState = .failed(
                        "Fresh Review data was unavailable. Existing Reviews were kept."
                    )
                    return
                }
                self.reviews = Self.mergeDeferredRecords(
                    baseline: baselineReviews,
                    current: self.reviews,
                    loaded: loaded.records
                )
                self.reviewLoadState = .loaded
                if let selectedReviewId = self.selectedReviewId,
                   !self.reviews.contains(where: { $0.id == selectedReviewId }) {
                    self.selectedReviewId = nil
                }
            } catch is CancellationError {
                return
            } catch {
                guard let self, self.workspaceReloadGeneration == generation else { return }
                self.reviewLoadState = .failed(error.localizedDescription)
            }
        }

        postReadyRetrySyncTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.workspaceReloadGeneration == generation {
                    self.postReadyRetrySyncTask = nil
                }
            }
            guard let self,
                  self.workspaceReloadGeneration == generation,
                  self.phase == .ready,
                  !Task.isCancelled else {
                return
            }
            _ = await self.retrySync(projectId: self.activeProjectId)
        }

    }

    private func applyLocalAgentAdapterResult(_ result: LocalAgentAdapterReconciliationResult) {
        let nextErrorMessage = Self.errorMessageAfterUpdatingLocalAgentAdapters(
            currentErrorMessage: errorMessage,
            previous: .init(
                conflicts: legacyAgentAdapterConflicts,
                inspectionWarning: legacyAgentAdapterInspectionWarning
            ),
            next: result
        )
        legacyAgentAdapterConflicts = result.conflicts
        legacyAgentAdapterInspectionWarning = result.inspectionWarning
        errorMessage = nextErrorMessage
    }

    nonisolated static func errorMessageAfterUpdatingLocalAgentAdapters(
        currentErrorMessage: String?,
        previous: LocalAgentAdapterReconciliationResult,
        next: LocalAgentAdapterReconciliationResult
    ) -> String? {
        let previousWarning = localAgentAdapterWarning(previous)
        guard currentErrorMessage == nil || currentErrorMessage == previousWarning else {
            return currentErrorMessage
        }
        return localAgentAdapterWarning(next)
    }

    nonisolated static func localAgentAdapterWarning(
        _ result: LocalAgentAdapterReconciliationResult
    ) -> String? {
        var messages: [String] = []
        if let inspectionWarning = result.inspectionWarning {
            messages.append(inspectionWarning)
        }
        let visible = result.conflicts.prefix(3).map { conflict in
            let adapter = conflict.adapter == .claudeCode ? "Claude Code" : conflict.adapter.rawValue
            return "\(adapter) \(conflict.scope) integration at \(conflict.targetRoot): \(conflict.message)"
        }
        messages.append(contentsOf: visible)
        if result.conflicts.count > visible.count {
            messages.append("\(result.conflicts.count - visible.count) more legacy integrations need review.")
        }
        return messages.isEmpty ? nil : messages.joined(separator: "\n")
    }

    nonisolated static func combinedAgentAdapterWarning(
        _ managedWarning: String?,
        _ legacyWarning: String?
    ) -> String? {
        let warnings = [managedWarning, legacyWarning].compactMap { $0 }
        return warnings.isEmpty ? nil : warnings.joined(separator: "\n")
    }

    private func refreshDraft(_ draftId: String) async throws {
        let detail = try await daemon.draft(draftId)
        let mapped = WorkspaceLoader.mapDraft(detail, resources: resources)
        if let index = drafts.firstIndex(where: { $0.id == mapped.id }) {
            drafts[index] = mapped
        } else {
            drafts.append(mapped)
        }
        refreshDocumentTabs(for: mapped.targetId ?? mapped.id)
    }

    private func remapDrafts(
        projectId: String,
        workspaceGeneration: UUID
    ) async throws {
        let projectDrafts = drafts.filter { $0.projectId == projectId }
        let originalById = Dictionary(
            projectDrafts.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        let targetIds = Set(projectDrafts.compactMap(\.targetId))
        let baselines = resources.filter { targetIds.contains($0.id) && !$0.contentLoaded }
        let loader = WorkspaceLoader(daemon: daemon, bootstrap: bootstrap, server: server)
        let loadedBaselines = try await concurrentMap(baselines) { try await loader.loadContent(for: $0) }
        guard workspaceReloadGeneration == workspaceGeneration else { return }
        for loaded in loadedBaselines {
            installLoadedResourceIfCurrent(loaded)
        }
        let resourceSnapshot = resources
        let mapped = try await concurrentMap(projectDrafts) { draft in
            WorkspaceLoader.mapDraft(
                try await self.daemon.draft(draft.id),
                resources: resourceSnapshot
            )
        }
        guard workspaceReloadGeneration == workspaceGeneration else { return }
        for candidate in mapped {
            guard let original = originalById[candidate.id],
                  let index = drafts.firstIndex(where: { $0.id == candidate.id }),
                  drafts[index] == original else { continue }
            drafts[index] = candidate
            refreshDocumentTabs(for: candidate.targetId ?? candidate.id)
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

    private func refreshDraftInventory(
        includeFailed: Bool,
        generation: UUID
    ) async {
        guard workspaceReloadGeneration == generation,
              phase == .ready,
              !Task.isCancelled else {
            return
        }

        let inventory: [DaemonDraftSummary]
        do {
            inventory = try await WorkspaceLoader.listAllDraftSummaries { query in
                try await self.daemon.listDrafts(query)
            }
        } catch is CancellationError {
            return
        } catch {
            guard workspaceReloadGeneration == generation, phase == .ready else { return }
            draftInventoryLoadState = .failed(
                "Couldn’t refresh Drafts. \(error.localizedDescription)"
            )
            return
        }

        guard workspaceReloadGeneration == generation, phase == .ready, !Task.isCancelled else {
            return
        }
        let plan = Self.draftInventoryPlan(
            summaries: inventory,
            currentDrafts: drafts,
            includeFailed: includeFailed
        )
        let pendingDraftIds = Set(drafts.compactMap { draft in
            let itemId = draft.targetId ?? draft.id
            if pendingDocumentSaves[
                .init(projectId: draft.projectId, itemId: itemId)
            ] != nil {
                return draft.id
            }
            return nil
        })
        drafts.removeAll {
            plan.terminalIds.contains($0.id) && !pendingDraftIds.contains($0.id)
        }
        pruneOrphanedMemoryTabs()

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
        let baselines = resources.filter { targetIds.contains($0.id) && !$0.contentLoaded }
        let loader = WorkspaceLoader(daemon: daemon, bootstrap: bootstrap, server: server)

        do {
            let loadedBaselines = try await concurrentMap(baselines) {
                try await loader.loadContent(for: $0)
            }
            guard workspaceReloadGeneration == generation,
                  phase == .ready,
                  !Task.isCancelled else {
                return
            }
            for loaded in loadedBaselines {
                installLoadedResourceIfCurrent(loaded)
            }
        } catch is CancellationError {
            return
        } catch {
            guard workspaceReloadGeneration == generation, phase == .ready else { return }
            draftInventoryLoadState = .failed(
                "Couldn’t refresh Draft source files. \(error.localizedDescription)"
            )
            return
        }

        let resourceSnapshot = resources
        let mappedDrafts: [LocalDraft]
        do {
            mappedDrafts = try await concurrentMap(summaries) { summary in
                WorkspaceLoader.mapDraft(
                    try await self.daemon.draft(summary.draftId),
                    resources: resourceSnapshot
                )
            }
        } catch is CancellationError {
            return
        } catch {
            guard workspaceReloadGeneration == generation, phase == .ready else { return }
            draftInventoryLoadState = .failed(
                "Couldn’t refresh Draft details. \(error.localizedDescription)"
            )
            return
        }

        guard workspaceReloadGeneration == generation, phase == .ready, !Task.isCancelled else {
            return
        }
        for mapped in mappedDrafts {
            if let index = drafts.firstIndex(where: { $0.id == mapped.id }) {
                guard originalById[mapped.id] == drafts[index] else { continue }
                drafts[index] = mapped
            } else if originalById[mapped.id] == nil {
                drafts.append(mapped)
            }
            refreshDocumentTabs(for: mapped.targetId ?? mapped.id)
        }
        draftInventoryLoadState = .loaded
    }

    private func refreshProjectFromServer(
        projectId: String,
        projectName: String,
        generation: UUID
    ) async {
        let workspaceGeneration = workspaceReloadGeneration
        do {
            guard let observedProject = projects.first(where: { $0.id == projectId }),
                  observedProject.name == projectName else { return }
            let loaded = try await WorkspaceLoader(
                daemon: daemon,
                bootstrap: bootstrap,
                server: server
            ).loadProjectWithMetadata(id: projectId, name: projectName)
            guard projectSelectionGeneration == generation,
                  activeProjectId == projectId,
                  projects.first(where: { $0.id == projectId }) == observedProject,
                  !loaded.hasStaleServerResponse else { return }
            clearStaleResourceState(for: projectId)
            if let index = projects.firstIndex(where: { $0.id == projectId }) {
                projects[index] = loaded.state
            }
            replaceProjectResources(projectId: projectId, with: loaded.resources)
            try await remapDrafts(
                projectId: projectId,
                workspaceGeneration: workspaceGeneration
            )
            resolveBackgroundError(.projectRefresh(projectId: projectId))
        } catch is CancellationError {
            return
        } catch {
            guard workspaceReloadGeneration == workspaceGeneration,
                  projectSelectionGeneration == generation,
                  phase == .ready,
                  activeProjectId == projectId else {
                return
            }
            presentBackgroundError(
                "Couldn’t refresh \(projectName). Existing content is still available. "
                    + error.localizedDescription,
                source: .projectRefresh(projectId: projectId)
            )
        }
    }

    private func refreshReview(_ reviewId: String) async throws {
        let detail: ReviewDetail = try await server.get("/api/v1/reviews/\(reviewId)")
        replaceReview(with: WorkspaceLoader.mapReview(detail))
    }

    func replaceReview(with review: ReviewRecord) {
        if let index = reviews.firstIndex(where: { $0.id == review.id }) {
            reviews[index] = review
        } else {
            reviews.insert(review, at: 0)
        }
    }

    private func loadCommit(_ commitId: String?) async throws -> CommitPayload? {
        guard let commitId else { return nil }
        return try await server.get("/api/v1/commits/\(commitId)")
    }

    private static func refETag(_ commitId: String?) -> String {
        "\"\(commitId ?? "ref-none")\""
    }

    private func uniqueDefaultPath(
        for kind: MemoryKind,
        scope: MemoryScope,
        authoritativeOrgResources: [MemoryResource]? = nil,
        projectId: String? = nil
    ) -> String {
        let base: String
        switch kind {
        case .context: base = "untitled.md"
        case .rules: base = "untitled.md"
        case .workflows: base = "workflow/untitled.md"
        }
        let scopedResources: [MemoryResource]
        let scopedDrafts: [LocalDraft]
        if scope == .org, let authoritativeOrgResources {
            scopedResources = authoritativeOrgResources
            let carrierProjectId = projectId ?? activeProjectId
            scopedDrafts = drafts.filter {
                $0.scope == .org && $0.projectId == carrierProjectId
            }
        } else if scope == .project, let activeProjectId {
            scopedResources = Self.memoryTreeResources(
                resources,
                activeProjectId: activeProjectId,
                selectedOrgResourceIds: activeProject?.selectedOrgResourceIds ?? []
            )
            scopedDrafts = Self.memoryTreeDrafts(drafts, activeProjectId: activeProjectId)
        } else {
            scopedResources = resources.filter { $0.scope == scope }
            scopedDrafts = drafts.filter { $0.scope == scope }
        }
        // Memory kind is unified on the wire. Context/Rules/Workflow are
        // creation presets, not separate path namespaces, so every effective
        // resource and LocalDraft participates in collision avoidance.
        let paths = Set(scopedResources.map(\.document.path))
            .union(scopedDrafts.map(\.document.path))
        return Self.uniqueDefaultPath(base: base, occupiedPaths: paths)
    }

    nonisolated static func uniqueDefaultPath(
        base: String,
        occupiedPaths: Set<String>
    ) -> String {
        guard occupiedPaths.contains(base) else { return base }
        let extensionStart = base.lastIndex(of: ".") ?? base.endIndex
        let stem = String(base[..<extensionStart])
        let suffix = String(base[extensionStart...])
        var index = 2
        while occupiedPaths.contains("\(stem)-\(index)\(suffix)") { index += 1 }
        return "\(stem)-\(index)\(suffix)"
    }

    private func normalizedRepositoryPaths(_ paths: [String]) -> [String] {
        Array(
            Set(
                paths.compactMap { path in
                    let normalized = path.trimmingCharacters(in: .whitespacesAndNewlines)
                    return normalized.isEmpty ? nil : URL(fileURLWithPath: normalized).standardized.path
                }
            )
        )
        .sorted()
    }

    private func bundledAgentRuntimePath() throws -> String {
        try AppBundleRuntimeLocation.requireStable(Bundle.main.bundleURL)
        guard let path = Bundle.main.resourceURL?.appending(path: "clumsiesd").path,
              FileManager.default.isExecutableFile(atPath: path) else {
            throw ProjectSetupError.bundledAgentRuntimeMissing
        }
        return path
    }

    nonisolated static func defaultDocument(
        kind: MemoryKind,
        path: String
    ) -> EditableMemoryDocument {
        switch kind {
        case .context:
            .init(title: "Untitled", path: path, body: "# Untitled\n")
        case .rules:
            .init(title: "Untitled rule", path: path, body: "# Untitled rule\n")
        case .workflows:
            .init(
                title: "Untitled workflow",
                path: path,
                body: "# Untitled workflow\n"
            )
        }
    }

    private func daemonContent(kind: MemoryKind, document: EditableMemoryDocument) -> DaemonDraftContent {
        .init(description: nil, content: document.body)
    }

    private func validatePath(kind: MemoryKind, path: String) throws {
        let segments = path.split(separator: "/", omittingEmptySubsequences: false)
        if path.isEmpty
            || path.hasPrefix("/")
            || path.hasSuffix("/")
            || segments.contains(where: { $0.isEmpty || $0 == "." || $0 == ".." }) {
            throw MemoryValidationError.invalidPath("Use a normalized relative path with / separators.")
        }
        if kind == .workflows && !path.hasPrefix("workflow/") {
            throw MemoryValidationError.invalidPath("Workflow paths must use the workflow/ namespace.")
        }
        if kind == .rules && path.lowercased().hasPrefix("workflow/") {
            throw MemoryValidationError.invalidPath("Rule paths cannot use the workflow/ namespace.")
        }
    }

    private func validate(kind: MemoryKind, document: EditableMemoryDocument) throws {
        try validatePath(kind: kind, path: document.path)
        if kind == .rules && document.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            throw MemoryValidationError.emptyRule
        }
    }
}

private struct PendingDocumentSave {
    let item: MemoryListItem
    let document: EditableMemoryDocument
    let generation: UUID
}

private struct PendingBundleSave {
    let bundle: PersonalBundle
    let name: String
    let description: String
    let resourceIds: Set<String>
    let generation: UUID
}
