import Combine
import Foundation

@MainActor
final class WorkspaceCoordinator {
    let agents: AgentIntegrationService
    let bundleSelection: BundlesModel
    let bundles: BundleStore
    let catalog: MemoryCatalog
    let context: WorkspaceContext
    let edits: DraftStore
    let feedback: WorkspaceFeedback
    let inbox: InboxStore
    let memory: MemoryModel
    let navigation: WorkspaceNavigation
    let projects: ProjectService
    let reconciliation: DraftReconciliationService
    let refresh: DaemonSyncService
    let reviews: ReviewsModel
    let sessions: DocumentSessions
    let sync: MemorySyncService
    private var observations: Set<AnyCancellable> = []

    init(storeDraft: (@Sendable (DaemonDraftOperationRequest) async throws -> DaemonDraftOperationResponse)? = nil) {
        let context = WorkspaceContext()
        self.context = context
        let catalog = MemoryCatalog(context: context)
        self.catalog = catalog
        self.inbox = InboxStore(context: context, catalog: catalog)
        let feedback = WorkspaceFeedback(context: context)
        self.feedback = feedback
        let sessions = DocumentSessions(context: context)
        self.sessions = sessions
        let agents = AgentIntegrationService(context: context, feedback: feedback)
        self.agents = agents
        let bundles = BundleStore(catalog: catalog, context: context, feedback: feedback)
        self.bundles = bundles
        let edits = DraftStore(catalog: catalog, context: context, feedback: feedback, sessions: sessions, storeDraft: storeDraft)
        self.edits = edits
        let refresh = DaemonSyncService(context: context, feedback: feedback)
        self.refresh = refresh
        let sync = MemorySyncService(catalog: catalog, context: context, feedback: feedback)
        self.sync = sync
        let bundleSelection = BundlesModel(bundles: bundles)
        self.bundleSelection = bundleSelection
        let navigation = WorkspaceNavigation(catalog: catalog, context: context, edits: edits, feedback: feedback, sessions: sessions)
        self.navigation = navigation
        let projects = ProjectService(agents: agents, bundles: bundles, catalog: catalog, context: context, edits: edits, refresh: refresh, sessions: sessions)
        self.projects = projects
        let reconciliation = DraftReconciliationService(catalog: catalog, context: context, edits: edits, refresh: refresh, sessions: sessions)
        self.reconciliation = reconciliation
        let memory = MemoryModel(catalog: catalog, context: context, edits: edits, feedback: feedback, navigation: navigation, projects: projects, reconciliation: reconciliation, sessions: sessions)
        self.memory = memory
        let reviews = ReviewsModel(context: context, edits: edits, feedback: feedback, navigation: navigation, reconciliation: reconciliation, sessions: sessions)
        self.reviews = reviews
        feedback.isShowingOrganizationMemory = { [weak navigation] in
            navigation?.selectedSection == .memory
        }
        context.projectSelectionChanges.sink { [weak feedback] in
            feedback?.clearIrrelevantScopedErrorPresentation()
        }.store(in: &observations)
        catalog.documentsChanged.merge(with: edits.documentsChanged).sink { [weak navigation] in
            navigation?.pruneOrphanedMemoryTabs()
            navigation?.refreshAllDocumentTabs()
        }.store(in: &observations)
        edits.didSaveDocument.sink { [weak navigation] id in
            navigation?.selectedItemId = id
        }.store(in: &observations)
        edits.didDiscardDocument.sink { [weak navigation] in
            navigation?.selectedItemId = nil
        }.store(in: &observations)
        sessions.didFinish.sink { [weak navigation] key in
            if navigation?.pendingDocumentCommand?.sessionKey == key {
                navigation?.pendingDocumentCommand = nil
            }
        }.store(in: &observations)
        refresh.onRetryCompleted = { [weak self] in
            guard let self else { return }
            await self.refresh.refreshSyncStatus()
            guard !Task.isCancelled else { return }
            await refreshSynchronizedWorkspaceData()
        }
        reconciliation.onReconciled = { [weak self] in
            await self?.reload(allowsDuringDocumentReconciliation: true)
        }
        reviews.onMerged = { [weak self] in await self?.reload() }
    }

    var hasPendingChanges: Bool {
        edits.hasPendingChanges || bundles.hasPendingChanges
    }

    func start() {
        Task { await self.reload() }
    }

    func reload(allowsDuringDocumentReconciliation: Bool = false) async {
        guard !context.isSigningOut else { return }
        guard allowsDuringDocumentReconciliation
                || (sessions.applyingDocumentReconciliationSessions.isEmpty
                    && sessions.standaloneReconciliationActivityIds.isEmpty) else {
            return
        }
        guard context.loadingProjectId == nil, !context.isSwitchingMemoryContext else { return }
        if context.phase == .ready, hasPendingChanges, !(await flushPendingChanges()) {
            return
        }
        // A project selection may have started while pending edits were
        // flushing. Let that serialized intent finish before a later reload.
        guard context.loadingProjectId == nil, !context.isSwitchingMemoryContext else { return }
        cancelPostReadyWork()
        feedback.resetBackgroundErrorPresentation()
        let generation = UUID()
        context.workspaceReloadGeneration = generation
        let hadLoadedWorkspace = context.account != nil
        context.phase = .loading
        feedback.errorMessage = nil
        do {
            let snapshot = try await WorkspaceLoader(
                daemon: context.daemon,
                bootstrap: context.bootstrap,
                server: context.server
            ).load { [weak self] result in
                guard self?.context.workspaceReloadGeneration == generation else { return }
                self?.agents.applyLocalAgentAdapterResult(result)
            }
            guard context.workspaceReloadGeneration == generation else { return }
            let sameAuthority = WorkspaceLoadPolicy.preservesDeferredAuthority(
                currentAccount: context.account,
                currentOrganization: context.organization,
                nextAccount: snapshot.account,
                nextOrganization: snapshot.organization
            )
            let snapshotWasStale = snapshot.runtime.serverDataSource == "stale"
            if WorkspaceLoadPolicy.rejectsStaleAuthorityChange(
                hadLoadedWorkspace: hadLoadedWorkspace,
                sameAuthority: sameAuthority,
                snapshotWasStale: snapshotWasStale
            ) {
                clearAuthorityScopedWorkspace()
                context.phase = .failed(
                    String(localized: "Fresh account data is required before switching workspaces. The previous account workspace was cleared.")
                )
                return
            }
            let preservesLoadedWorkspace = hadLoadedWorkspace && sameAuthority
            // Stale-cache data keeps a cold start usable while offline, but it
            // must never replace a newer generation already held in memory.
            guard !preservesLoadedWorkspace || !snapshotWasStale else {
                context.phase = .ready
                startPostReadyWork(
                    generation: generation,
                    requiresFreshData: true,
                    baseSnapshotWasStale: true
                )
                return
            }
            apply(snapshot)
            context.phase = .ready
            startPostReadyWork(
                generation: generation,
                requiresFreshData: WorkspaceLoadPolicy.deferredLoadRequiresFreshData(
                    hadLoadedWorkspace: hadLoadedWorkspace
                ),
                baseSnapshotWasStale: false
            )
        } catch WorkspaceLoadError.authenticationRequired {
            guard context.workspaceReloadGeneration == generation else { return }
            context.phase = .authenticationRequired
        } catch {
            guard context.workspaceReloadGeneration == generation else { return }
            let messages = [error.localizedDescription, feedback.errorMessage]
                .compactMap { $0 }
                .filter { !$0.isEmpty }
            context.phase = .failed(messages.joined(separator: "\n\n"))
        }
    }

    func showOrgMemory() async {
        guard context.phase == .ready else { return }
        guard context.activeProjectId != nil || context.loadingProjectId != nil else { return }
        guard sessions.canCommitMemoryContextSwitch else {
            feedback.errorMessage = String(localized: "Finish or cancel the active document Sync before switching Memory context.")
            return
        }
        // Showing Org supersedes every in-flight Project selection. A stale
        // daemon reply must not be allowed to switch the UI back afterward.
        let generation = UUID()
        context.projectSelectionGeneration = generation
        context.loadingProjectId = nil
        context.isSwitchingMemoryContext = true
        navigation.clearPendingDocumentSessionPresentation()
        defer {
            if self.context.projectSelectionGeneration == generation {
                self.context.isSwitchingMemoryContext = false
            }
        }
        guard await edits.flushPendingDocumentChanges(),
              context.projectSelectionGeneration == generation else { return }
        guard sessions.canCommitMemoryContextSwitch else {
            feedback.errorMessage = String(localized: "Finish or cancel the active document Sync before switching Memory context.")
            return
        }
        navigation.clearPendingDocumentSessionPresentation()
        context.activeProjectId = nil
        catalog.refreshVisibleStaleResourceIds()
        navigation.showsProjectSettings = false
        navigation.selectedItemId = nil
        let tab = navigation.visibleTabs.last
        navigation.activeTabId = tab?.id
    }

    func selectProject(_ projectId: String) async {
        guard context.phase == .ready else { return }
        guard let project = context.projects.first(where: { $0.id == projectId }) else { return }
        if projectId == context.activeProjectId,
           project.isLoaded,
           context.loadingProjectId == nil,
           !context.isSwitchingMemoryContext {
            return
        }
        guard sessions.canCommitMemoryContextSwitch else {
            feedback.errorMessage = String(localized: "Finish or cancel the active document Sync before switching Memory context.")
            return
        }
        let generation = UUID()
        let workspaceGeneration = context.workspaceReloadGeneration
        // Publish the newest intent before the first suspension point. This
        // closes the window where two clicks could both start a daemon-side
        // selection while a pending document save was flushing.
        context.projectSelectionGeneration = generation
        context.loadingProjectId = projectId
        context.isSwitchingMemoryContext = true
        navigation.clearPendingDocumentSessionPresentation()
        defer {
            if self.context.projectSelectionGeneration == generation {
                self.context.loadingProjectId = nil
                self.context.isSwitchingMemoryContext = false
            }
        }
        guard await edits.flushPendingDocumentChanges(),
              context.projectSelectionGeneration == generation else { return }
        guard sessions.canCommitMemoryContextSwitch else {
            feedback.errorMessage = String(localized: "Finish or cancel the active document Sync before switching Memory context.")
            return
        }
        do {
            let selectedLatestIntent = try await context.projectSelectionSideEffectGate.run {
                guard self.context.projectSelectionGeneration == generation else { return false }
                _ = try await self.context.daemon.selectProject(projectId)
                return true
            }
            guard selectedLatestIntent,
                  context.projectSelectionGeneration == generation else { return }
            guard sessions.canCommitMemoryContextSwitch else {
                feedback.errorMessage = String(localized: "Finish or cancel the active document Sync before switching Memory context.")
                return
            }
            navigation.clearPendingDocumentSessionPresentation()
            context.activeProjectId = projectId
            catalog.refreshVisibleStaleResourceIds()
            let tab = navigation.visibleTabs.last
            navigation.activeTabId = tab?.id
            navigation.selectedItemId = tab?.itemId

            let loader = WorkspaceLoader(daemon: context.daemon, bootstrap: context.bootstrap, server: context.server)
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
            guard context.projectSelectionGeneration == generation else { return }
            if let loadedProject {
                if let index = context.projects.firstIndex(where: { $0.id == projectId }) {
                    context.projects[index] = loadedProject.state
                }
                catalog.replaceProjectResources(projectId: projectId, with: loadedProject.resources)
                try await edits.remapDrafts(
                    projectId: projectId,
                    workspaceGeneration: workspaceGeneration
                )
            }
            guard context.projectSelectionGeneration == generation else { return }
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
            guard context.projectSelectionGeneration == generation else { return }
            feedback.errorMessage = error.localizedDescription
        }
    }

    func prepareWorkspaceIndex(includeContent: Bool) async {
        let generation = context.workspaceReloadGeneration
        while catalog.isPreparingWorkspaceIndex {
            try? await Task.sleep(for: .milliseconds(100))
            guard context.workspaceReloadGeneration == generation else { return }
        }
        guard context.workspaceReloadGeneration == generation else { return }
        let needsProjects = context.projects.contains { !$0.isLoaded }
        let needsContent = includeContent && catalog.resources.contains { !$0.contentLoaded }
        guard needsProjects || needsContent else { return }

        catalog.isPreparingWorkspaceIndex = true
        defer {
            if self.context.workspaceReloadGeneration == generation {
                self.catalog.isPreparingWorkspaceIndex = false
            }
        }
        do {
            let loader = WorkspaceLoader(daemon: context.daemon, bootstrap: context.bootstrap, server: context.server)
            let unloadedProjects = context.projects.filter { !$0.isLoaded }
            let loadedProjects = try await concurrentMap(unloadedProjects, maxConcurrent: 4) { project in
                try await loader.loadProject(id: project.id, name: project.name)
            }
            guard context.workspaceReloadGeneration == generation else { return }
            for loaded in loadedProjects {
                catalog.clearStaleResourceState(for: loaded.state.id)
                if let index = context.projects.firstIndex(where: { $0.id == loaded.state.id }) {
                    context.projects[index] = loaded.state
                }
                catalog.replaceProjectResources(projectId: loaded.state.id, with: loaded.resources)
            }

            if includeContent {
                let unloadedResources = catalog.resources.filter { !$0.contentLoaded }
                let loadedResources = try await concurrentMap(unloadedResources) {
                    try await loader.loadContent(for: $0)
                }
                guard context.workspaceReloadGeneration == generation else { return }
                for loaded in loadedResources {
                    catalog.installLoadedResourceIfCurrent(loaded)
                }
            }

            for projectId in Set(edits.drafts.map(\.projectId)) {
                try await edits.remapDrafts(
                    projectId: projectId,
                    workspaceGeneration: generation
                )
                guard context.workspaceReloadGeneration == generation else { return }
            }
        } catch is CancellationError {
            return
        } catch {
            guard context.workspaceReloadGeneration == generation else { return }
            feedback.errorMessage = error.localizedDescription
        }
    }

    func reveal(_ item: MemoryListItem) async {
        navigation.selectedSection = .memory
        navigation.selectedKind = item.kind
        if let projectId = item.projectContextId {
            await selectProject(projectId)
            guard context.activeProjectId == projectId else { return }
        } else if item.scope == .project, let projectId = item.projectId {
            await selectProject(projectId)
            guard context.activeProjectId == projectId else { return }
        } else if context.activeProjectId != nil,
                  !(context.activeProject?.selectedOrgResourceIds.contains(item.id) ?? false) {
            await showOrgMemory()
            guard context.activeProjectId == nil else { return }
        }
        navigation.open(item)
    }

    func signOut() async {
        guard !context.isSigningOut else { return }
        context.isSigningOut = true
        let priorPhase = context.phase
        context.phase = .loading
        defer { self.context.isSigningOut = false }
        guard await flushPendingChanges() else {
            context.phase = priorPhase
            return
        }
        context.workspaceReloadGeneration = UUID()
        cancelPostReadyWork()
        WorkspaceLoadPolicy.invalidateWorkspaceTransitionState(
            generation: &context.projectSelectionGeneration,
            loadingProjectId: &context.loadingProjectId,
            isSwitchingMemoryContext: &context.isSwitchingMemoryContext,
            isPreparingWorkspaceIndex: &catalog.isPreparingWorkspaceIndex,
            orgResourceRefreshGeneration: &catalog.orgResourceRefreshGeneration
        )
        _ = try? await context.server.raw(method: "DELETE", path: "/api/v1/auth/session")
        do {
            _ = try await context.projectSelectionSideEffectGate.run {
                try await self.context.daemon.replaceProjectConfig(
                    .init(
                        serverUrl: ClumsiesIdentifiers.serverURL.absoluteString,
                        projectId: nil,
                        accessToken: nil,
                        refreshToken: nil
                    )
                )
            }
            clearAuthorityScopedWorkspace()
            context.phase = .authenticationRequired
        } catch {
            feedback.errorMessage = error.localizedDescription
            context.phase = .failed(error.localizedDescription)
        }
    }

    func flushPendingChanges() async -> Bool {
        guard await edits.flushPendingDocumentChanges() else { return false }
        do {
            try await bundles.flushPendingChanges()
            return true
        } catch {
            feedback.errorMessage = error.localizedDescription
            return false
        }
    }

    func runRefreshLoop() async {
        let clock = ContinuousClock()
        var nextSynchronizedDataRefresh = clock.now
        while !Task.isCancelled {
            if context.phase == .ready {
                await refresh.refreshSyncStatus()
                guard !Task.isCancelled else { return }
                if clock.now >= nextSynchronizedDataRefresh {
                    await refreshSynchronizedWorkspaceData()
                    await inbox.refresh()
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

    func refreshSynchronizedWorkspaceData() async {
        guard context.phase == .ready, !refresh.isRefreshingSynchronizedWorkspaceData else { return }
        refresh.isRefreshingSynchronizedWorkspaceData = true
        defer { self.refresh.isRefreshingSynchronizedWorkspaceData = false }
        let generation = context.workspaceReloadGeneration
        let projectId = context.activeProjectId
        await self.sync.refreshOrgResourcesIfNeeded(isActive: { self.navigation.selectedSection == .memory })
        guard context.workspaceReloadGeneration == generation,
              context.activeProjectId == projectId,
              context.phase == .ready,
              !Task.isCancelled,
              let sync = refresh.runtime?.sync else {
            return
        }
        if edits.draftInventoryLoadTask == nil {
            await edits.refreshDraftInventory(
                includeFailed: sync.pendingOperationCount > 0
                    || sync.failedOperationCount > 0,
                generation: generation
            )
        }
        guard context.workspaceReloadGeneration == generation,
              context.activeProjectId == projectId,
              context.phase == .ready,
              !Task.isCancelled else {
            return
        }
        await self.sync.refreshStaleResourcesIfNeeded(sync: sync)
    }

    func clearAuthorityScopedWorkspace() {
        cancelPostReadyWork()
        refresh.resetAuthority()
        feedback.resetBackgroundErrorPresentation()
        edits.resetAuthority()
        inbox.reset()
        bundles.resetAuthority()
        reviews.resetAuthority()
        sessions.resetAuthority()
        navigation.resetAuthority()
        bundleSelection.selectedBundleId = nil
        catalog.resetAuthority()
        context.resetAuthority()
    }

    func apply(_ snapshot: WorkspaceSnapshot) {
        let preservesAuthority = WorkspaceLoadPolicy.preservesDeferredAuthority(
            currentAccount: context.account, currentOrganization: context.organization,
            nextAccount: snapshot.account, nextOrganization: snapshot.organization
        )
        if !preservesAuthority {
            // Keep the reload token: this snapshot is the result of that load.
            let generation = context.workspaceReloadGeneration
            clearAuthorityScopedWorkspace()
            context.workspaceReloadGeneration = generation
        } else {
            context.invalidateProjectSelection()
        }
        sessions.resetDocumentTasks()
        navigation.clearPendingDocumentSessionPresentation()
        context.applyAuthority(snapshot)
        let accessibleProjectIds = Set(context.projects.map(\.id))
        edits.retainAccessibleProjects(accessibleProjectIds)
        reviews.retainAccessibleProjects(accessibleProjectIds)
        catalog.apply(snapshot)
        navigation.applyWorkspace()
        refresh.applyRuntime(snapshot.runtime)
        inbox.prepare(serverURL: snapshot.runtime.health.serverUrl)
        agents.applyLocalAgentAdapterResult(.init(
            conflicts: snapshot.legacyAgentAdapterConflicts,
            inspectionWarning: snapshot.legacyAgentAdapterInspectionWarning
        ))
    }

    private func cancelPostReadyWork() {
        edits.cancelLoading()
        bundles.cancelLoading()
        reviews.cancelLoading()
        agents.cancelLoading()
        refresh.cancelLoading()
    }

    private func startPostReadyWork(
        generation: UUID,
        requiresFreshData: Bool,
        baseSnapshotWasStale: Bool
    ) {
        guard context.workspaceReloadGeneration == generation, context.phase == .ready else { return }
        agents.startLoading(generation: generation)
        edits.startLoading(generation: generation, requiresFreshData: requiresFreshData, baseSnapshotWasStale: baseSnapshotWasStale)
        bundles.startLoading(generation: generation, requiresFreshData: requiresFreshData, baseSnapshotWasStale: baseSnapshotWasStale)
        reviews.startLoading(generation: generation, requiresFreshData: requiresFreshData, baseSnapshotWasStale: baseSnapshotWasStale)
        refresh.startLoading(generation: generation)
    }

    private func refreshProjectFromServer(
        projectId: String,
        projectName: String,
        generation: UUID
    ) async {
        let workspaceGeneration = context.workspaceReloadGeneration
        do {
            guard let observedProject = context.projects.first(where: { $0.id == projectId }),
                  observedProject.name == projectName else { return }
            let loaded = try await WorkspaceLoader(
                daemon: context.daemon,
                bootstrap: context.bootstrap,
                server: context.server
            ).loadProjectWithMetadata(id: projectId, name: projectName)
            guard context.projectSelectionGeneration == generation,
                  context.activeProjectId == projectId,
                  context.projects.first(where: { $0.id == projectId }) == observedProject,
                  !loaded.hasStaleServerResponse else { return }
            catalog.clearStaleResourceState(for: projectId)
            if let index = context.projects.firstIndex(where: { $0.id == projectId }) {
                context.projects[index] = loaded.state
            }
            catalog.replaceProjectResources(projectId: projectId, with: loaded.resources)
            try await edits.remapDrafts(
                projectId: projectId,
                workspaceGeneration: workspaceGeneration
            )
            feedback.resolveBackgroundError(.projectRefresh(projectId: projectId))
        } catch is CancellationError {
            return
        } catch {
            guard context.workspaceReloadGeneration == workspaceGeneration,
                  context.projectSelectionGeneration == generation,
                  context.phase == .ready,
                  context.activeProjectId == projectId else {
                return
            }
            feedback.presentBackgroundError(
                String(localized: "Couldn’t refresh \(projectName). Existing content is still available. ")
                    + error.localizedDescription,
                source: .projectRefresh(projectId: projectId)
            )
        }
    }
}
