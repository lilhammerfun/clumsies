import Combine
import Foundation

enum DocumentSessionCommand: Equatable, Sendable {
    case requestReview(sessionKey: MemoryDocumentSessionKey, draft: LocalDraft)
    case discardDraft(sessionKey: MemoryDocumentSessionKey, draft: LocalDraft)
    case moveToTrash(sessionKey: MemoryDocumentSessionKey)

    var sessionKey: MemoryDocumentSessionKey {
        switch self {
        case .requestReview(let sessionKey, _),
             .discardDraft(let sessionKey, _),
             .moveToTrash(let sessionKey):
            sessionKey
        }
    }
}

@MainActor
final class WorkspaceNavigation: ObservableObject {
    private let catalog: MemoryCatalog
    private let context: WorkspaceContext
    private let edits: DraftStore
    private let feedback: WorkspaceFeedback
    private let sessions: DocumentSessions
    var memoryItems: [MemoryListItem] {
        MemoryTreeProjection.items(
            resources: catalog.resources, drafts: edits.drafts,
            activeProjectId: context.activeProjectId,
            selectedOrgResourceIds: context.activeProject?.selectedOrgResourceIds ?? []
        )
    }

    init(catalog: MemoryCatalog, context: WorkspaceContext, edits: DraftStore, feedback: WorkspaceFeedback, sessions: DocumentSessions) {
        self.catalog = catalog
        self.context = context
        self.edits = edits
        self.feedback = feedback
        self.sessions = sessions
    }

    @Published var selectedSection: WorkspaceSection = .memory
    @Published var selectedKind: MemoryKind = .context
    @Published var selectedItemId: String?
    @Published var searchQuery = ""
    @Published var workspaceSearchFocusToken = UUID()
    @Published var reviewSearchFocusToken = UUID()
    @Published var showsProjectCreation = false
    @Published var showsProjectSettings = false
    @Published var sidebarExpanded = true
    @Published var tabs: [WorkbenchTab] = []
    @Published var activeTabId: String?
    @Published var navigationBackStack: [String] = []
    @Published var navigationForwardStack: [String] = []
    @Published var pendingDocumentCommand: DocumentSessionCommand?

    var selectedItem: MemoryListItem? {
        let itemId = activeVisibleTab?.itemId ?? selectedItemId
        return memoryItems.first { $0.id == itemId } ?? memoryItems.first
    }

    var visibleTabs: [WorkbenchTab] {
        tabs.filter { tab in
            guard tab.isVisible(in: self.selectedSection, projectId: self.context.activeProjectId) else {
                return false
            }
            guard self.selectedSection == .memory else { return true }
            let allowsUnresolved = self.context.phase != .ready || self.context.activeProject?.isLoaded == false
            guard let activeProjectId = self.context.activeProjectId else {
                return Self.orgMemoryTabIsAvailable(
                    itemId: tab.itemId,
                    resources: self.catalog.resources,
                    allowsUnresolved: allowsUnresolved
                )
            }
            guard tab.projectId == activeProjectId else { return false }
            return Self.memoryTabIsAvailable(
                itemId: tab.itemId,
                projectId: activeProjectId,
                selectedOrgResourceIds: self.context.activeProject?.selectedOrgResourceIds ?? [],
                resources: self.catalog.resources,
                drafts: self.edits.drafts,
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
            self.memoryTabIsAvailable(
                $0,
                projects: projects,
                resources: resources,
                drafts: drafts,
                allowsUnresolvedOrg: allowsUnresolvedOrg
            )
        }
    }

    var activeVisibleTab: WorkbenchTab? {
        visibleTabs.first { $0.id == self.activeTabId } ?? visibleTabs.last
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

    func clearPendingDocumentSessionPresentation() {
        pendingDocumentCommand = nil
    }

    func presentProjectCreation() {
        guard context.canCreateProject else { return }
        showsProjectCreation = true
    }

    func focusWorkspaceSearch() {
        workspaceSearchFocusToken = UUID()
    }

    func focusReviewSearch() {
        reviewSearchFocusToken = UUID()
    }

    func open(_ item: MemoryListItem, mode: WorkbenchTabMode? = nil) {
        guard let item = Self.memoryItemForViewContext(
            item,
            activeProjectId: context.activeProjectId
        ) else { return }
        showsProjectSettings = false
        let previousTabId = activeVisibleTab?.id
        // Project-local overlays need a separate session from the Org
        // authority view even when both address the same Org memory id.
        let tabProjectId = context.activeProjectId ?? (item.scope == .org ? nil : item.projectId)
        let existingMode = tabs.first {
            $0.section == self.selectedSection
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
        Task { await self.catalog.loadContentIfNeeded(item) }
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
            let candidate = self.tabs[index]
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

    func refreshDocumentTabs(for itemId: String) {
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

    func refreshAllDocumentTabs() {
        for itemId in Set(tabs.map(\.itemId)) {
            refreshDocumentTabs(for: itemId)
        }
    }

    /// Remove document sessions that no longer exist in their own view
    /// context. In particular, a clean selected Org memory stops belonging to
    /// Project P as soon as P removes it, even though the Org authority remains
    /// globally live. A P-bound LocalDraft still retains P's draft-only tab.
    func pruneOrphanedMemoryTabs() {
        let retainedTabs = Self.retainedMemoryTabs(
            tabs,
            projects: context.projects,
            resources: catalog.resources,
            drafts: edits.drafts
        )
        let retainedTabIds = Set(retainedTabs.map(\.id))
        let removedTabs = tabs.filter { !retainedTabIds.contains($0.id) }
        let removedTabIds = Set(removedTabs.map(\.id))
        guard !removedTabIds.isEmpty else { return }
        for tab in removedTabs {
            sessions.clearDocumentSynchronizationState(for: tab)
        }
        tabs = retainedTabs
        navigationBackStack.removeAll { removedTabIds.contains($0) }
        navigationForwardStack.removeAll { removedTabIds.contains($0) }
        if let activeTabId = activeTabId, removedTabIds.contains(activeTabId) {
            if let activeTab = removedTabs.first(where: { $0.id == activeTabId }) {
                if let sessionKey = sessions.documentSessionKey(for: activeTab) {
                    if pendingDocumentCommand?.sessionKey == sessionKey {
                        pendingDocumentCommand = nil
                    }
                }
            }
            self.activeTabId = nil
            selectedItemId = nil
        }
    }

    func item(for tab: WorkbenchTab) -> MemoryListItem? {
        if let resource = catalog.resources.first(where: { $0.id == tab.itemId }) {
            guard tab.projectId != nil || resource.scope == .org else { return nil }
            let draft = MemoryTreeProjection.memoryTabDraft(
                itemId: resource.id,
                projectId: tab.projectId,
                drafts: edits.drafts
            )
            let tabProject = tab.projectId.flatMap { projectId in
                self.context.projects.first { $0.id == projectId }
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
        if let draft = MemoryTreeProjection.memoryTabDraft(
            itemId: tab.itemId,
            projectId: tab.projectId,
            drafts: edits.drafts
        ) {
            let resource = draft.targetId.flatMap { target in self.catalog.resources.first { $0.id == target } }
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

    func closeTab(_ tab: WorkbenchTab) {
        if let key = sessions.documentSessionKey(for: tab),
           sessions.applyingDocumentReconciliationSessions.contains(key) {
            feedback.errorMessage = "Wait for the shared update to finish before closing this tab."
            return
        }
        guard let index = tabs.firstIndex(where: { $0.id == tab.id }) else { return }
        if let sessionKey = sessions.documentSessionKey(for: tab) {
            if pendingDocumentCommand?.sessionKey == sessionKey {
                pendingDocumentCommand = nil
            }
        }
        sessions.clearDocumentSynchronizationState(for: tab)
        let visibleIndex = visibleTabs.firstIndex(where: { $0.id == tab.id })
        tabs.remove(at: index)
        navigationBackStack.removeAll { $0 == tab.id }
        navigationForwardStack.removeAll { $0 == tab.id }
        if activeTabId == tab.id {
            let remaining = visibleTabs
            let replacementIndex = min(visibleIndex ?? remaining.count, remaining.count - 1)
            activeTabId = replacementIndex >= 0 ? remaining[replacementIndex].id : nil
            selectedItemId = tabs.first { $0.id == self.activeTabId }?.itemId
        }
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

    func resetAuthority() {
        selectedSection = .memory
        selectedItemId = nil
        tabs.removeAll()
        activeTabId = nil
        navigationBackStack.removeAll()
        navigationForwardStack.removeAll()
        showsProjectSettings = false
        clearPendingDocumentSessionPresentation()
    }

    func applyWorkspace() {
        pruneOrphanedMemoryTabs()
        refreshAllDocumentTabs()
        if context.projects.isEmpty {
            selectedSection = .memory
            showsProjectSettings = false
            tabs.removeAll()
            activeTabId = nil
            selectedItemId = nil
        }
    }
}
