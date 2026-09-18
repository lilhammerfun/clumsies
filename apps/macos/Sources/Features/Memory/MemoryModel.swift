import AppKit
import UniformTypeIdentifiers
import Combine
import Foundation

@MainActor
final class MemoryModel: ObservableObject {
    private let catalog: MemoryCatalog
    private let context: WorkspaceContext
    private let edits: DraftStore
    private let feedback: WorkspaceFeedback
    private let navigation: WorkspaceNavigation
    private let projects: ProjectService
    private let reconciliation: DraftReconciliationService
    private let sessions: DocumentSessions

    init(catalog: MemoryCatalog, context: WorkspaceContext, edits: DraftStore, feedback: WorkspaceFeedback, navigation: WorkspaceNavigation, projects: ProjectService, reconciliation: DraftReconciliationService, sessions: DocumentSessions) {
        self.catalog = catalog
        self.context = context
        self.edits = edits
        self.feedback = feedback
        self.navigation = navigation
        self.projects = projects
        self.reconciliation = reconciliation
        self.sessions = sessions
    }

    @Published var isExportingMemory = false

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
              let snapshot = catalog.staleResourceSnapshot(for: item),
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

    var visibleMemoryItems: [MemoryListItem] {
        guard navigation.selectedSection == .memory else { return [] }
        return MemoryTreeProjection.items(
            resources: catalog.resources, drafts: edits.drafts,
            activeProjectId: context.activeProjectId,
            selectedOrgResourceIds: context.activeProject?.selectedOrgResourceIds ?? []
        )
    }

    func canExportMemory(_ items: [MemoryListItem]) -> Bool {
        context.phase == .ready && !isExportingMemory && !context.isSwitchingMemoryContext
            && (context.activeProjectId == nil || context.activeProject?.isLoaded == true)
            && (context.activeProjectId == nil || edits.draftInventoryLoadState == .loaded)
            && items.contains { $0.draft?.isDeletion != true }
            && !items.contains { self.sessions.isSynchronizingDocument($0.id) }
    }

    func exportMemory(_ selection: [MemoryListItem]? = nil, name: String? = nil) {
        let items = selection ?? visibleMemoryItems
        guard canExportMemory(items) else { return }
        // Capture the view and pending editor text before the save panel can change context.
        let pendingDocuments = Dictionary(items.compactMap { item in
            self.edits.pendingDocument(for: item).map { (item.id, $0) }
        }, uniquingKeysWith: { _, latest in latest })
        let loader = WorkspaceLoader(daemon: context.daemon, bootstrap: context.bootstrap, server: context.server)
        let panel = NSSavePanel()
        panel.title = "Export Memory"
        panel.prompt = "Export"
        panel.message = "Export current files, including local draft edits. Deleted memories are excluded."
        let baseName = name ?? (selection == nil ? context.activeProject?.name : nil) ?? "Memory"
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
                    self.feedback.errorMessage = "Could Not Export Memory: \(error.localizedDescription)"
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

    func createMemory(kind: MemoryKind, scope: MemoryScope) async {
        do {
            _ = try await createMemoryDraft(kind: kind, scope: scope)
        } catch {
            feedback.errorMessage = error.localizedDescription
        }
    }

    private func createMemoryDraft(
        kind: MemoryKind,
        scope: MemoryScope
    ) async throws -> String? {
        guard scope == .org, edits.canCreateMemory(kind: kind, scope: scope),
              let projectId = context.activeProjectId else { return nil }
        let generation = context.workspaceReloadGeneration
        guard let authority = try await catalog.loadStableOrgAuthoritySnapshot(
            allowingEmptyHead: true
        ) else {
            throw ServerClientError.invalidResponse(
                "A fresh Organization Memory snapshot is required to create a Draft."
            )
        }
        guard context.workspaceReloadGeneration == generation else { return nil }
        return try await edits.withDraftMutation {
            guard WorkspaceContext.projectContextIsCurrent(
                isSwitchingMemoryContext: self.context.isSwitchingMemoryContext,
                activeProjectId: self.context.activeProjectId,
                expectedProjectId: projectId
            ), self.context.workspaceReloadGeneration == generation else { return nil }
            let path = self.uniqueDefaultPath(
                for: kind,
                scope: scope,
                authoritativeOrgResources: authority.resources,
                projectId: projectId
            )
            let document = Self.defaultDocument(kind: kind, path: path)
            let response = try await self.context.daemon.store(
                .init(
                    draftId: nil,
                    baseCommitId: authority.commitId,
                    projectId: projectId,
                    scope: .org,
                    resource: kind.daemonKind,
                    op: .create(
                        path: document.path,
                        content: self.edits.daemonContent(kind: kind, document: document),
                        description: nil
                    ),
                    source: .desktop
                )
            )
            guard WorkspaceContext.projectContextIsCurrent(
                isSwitchingMemoryContext: self.context.isSwitchingMemoryContext,
                activeProjectId: self.context.activeProjectId,
                expectedProjectId: projectId
            ), self.context.workspaceReloadGeneration == generation else { return nil }
            try await self.edits.refreshDraft(response.draftId)
            guard WorkspaceContext.projectContextIsCurrent(
                isSwitchingMemoryContext: self.context.isSwitchingMemoryContext,
                activeProjectId: self.context.activeProjectId,
                expectedProjectId: projectId
            ), self.context.workspaceReloadGeneration == generation else { return nil }
            self.navigation.selectedItemId = response.draftId
            return response.draftId
        }
    }

    /// Use loaded metadata for presentation; adoption revalidates shared authority before writing.
    func prepareMemoryGuidelines(
        projectId: String,
        refreshingAuthority: Bool = false
    ) async throws -> MemoryGuidelinesSetup {
        let generation = context.workspaceReloadGeneration
        let config = try await context.daemon.projectConfig()
        var authorityResources = catalog.resources.filter { $0.scope == .org }
        var authorityCommitId = catalog.orgRefCommitId
        if refreshingAuthority {
            guard let authority = try await catalog.loadStableOrgAuthoritySnapshot(allowingEmptyHead: true) else {
                throw ServerClientError.invalidResponse("Couldn’t check your organization's memory guidelines. Try again.")
            }
            authorityResources = authority.resources
            authorityCommitId = authority.commitId
            await edits.refreshDraftInventory(includeFailed: true, generation: generation)
        }
        try Task.checkCancellation()
        guard generation == context.workspaceReloadGeneration, context.phase == .ready,
              navigation.selectedSection == .memory,
              WorkspaceContext.projectContextIsCurrent(
                  isSwitchingMemoryContext: context.isSwitchingMemoryContext,
                  activeProjectId: context.activeProjectId,
                  expectedProjectId: projectId
              ) else { throw CancellationError() }
        switch edits.draftInventoryLoadState {
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
            .union(MemoryTreeProjection.memoryTreeDrafts(edits.drafts, activeProjectId: projectId).map(\.document.path))
        return setup
    }

    /// Recheck the offered action. A changed destination is presented again for the user to choose.
    func useMemoryGuidelines(_ offered: MemoryGuidelinesSetup) async throws -> MemoryGuidelinesSetup {
        let current = try await prepareMemoryGuidelines(projectId: offered.projectId, refreshingAuthority: true)
        guard current.hasSameDestination(as: offered) else { return current }
        let generation = context.workspaceReloadGeneration
        let itemId: String
        switch current.action {
        case .open(let id):
            itemId = id
        case .useOrganization(let resource):
            if !catalog.resources.contains(where: { $0.id == resource.id }) {
                catalog.resources.append(resource)
            }
            try await projects.addOrgMemories(resourceIds: [resource.id], toProject: current.projectId)
            itemId = resource.id
        case .createDefault:
            itemId = try await createMemoryGuidelines(current)
        }
        guard generation == context.workspaceReloadGeneration,
              context.activeProjectId == current.projectId,
              navigation.selectedSection == .memory else { throw CancellationError() }
        guard let item = visibleMemoryItems.first(where: { $0.id == itemId }) else {
            throw ServerClientError.invalidResponse("Memory guidelines were saved, but could not be opened. Refresh the project to open them.")
        }
        navigation.open(item, mode: .preview)
        return current
    }

    private func createMemoryGuidelines(_ setup: MemoryGuidelinesSetup) async throws -> String {
        let generation = context.workspaceReloadGeneration
        return try await edits.withDraftMutation {
            guard generation == self.context.workspaceReloadGeneration,
                  self.context.activeProjectId == setup.projectId,
                  self.edits.canCreateMemory(kind: .context, scope: .org) else { throw CancellationError() }
            let occupiedPaths = setup.occupiedPaths.union(
                MemoryTreeProjection.memoryTreeDrafts(self.edits.drafts, activeProjectId: setup.projectId).map(\.document.path)
            )
            let documents = try MemoryGuidelines.defaultDocuments(occupiedPaths: occupiedPaths)
            for document in documents { try self.edits.validate(kind: .context, document: document) }
            let responses = try await self.context.daemon.createMemoryDrafts(.init(
                projectId: setup.projectId,
                baseCommitId: setup.organizationCommitId,
                operations: documents.map {
                    .create(path: $0.path, content: self.edits.daemonContent(kind: .context, document: $0), description: nil)
                }
            ))
            guard generation == self.context.workspaceReloadGeneration,
                  self.context.activeProjectId == setup.projectId else { throw CancellationError() }
            let daemon = self.context.daemon
            let details = try await concurrentMap(responses) { try await daemon.draft($0.draftId) }
            guard generation == self.context.workspaceReloadGeneration,
                  self.context.activeProjectId == setup.projectId,
                  let guideline = responses.first else { throw CancellationError() }
            let created = details.map { WorkspaceLoader.mapDraft($0, resources: self.catalog.resources) }
            let createdIds = Set(created.map(\.id))
            self.edits.drafts = self.edits.drafts.filter { !createdIds.contains($0.id) } + created
            self.navigation.selectedItemId = guideline.draftId
            return guideline.draftId
        }
    }

    /// Pulls the latest shared version for one document:
    /// - a behind draft opens the shared-change review flow;
    /// - a stale resource (no local draft) is refreshed from the synced checkout.
    func syncDocument(_ item: MemoryListItem) {
        guard DocumentSessions.canStartDocumentSynchronization(
            isSwitchingMemoryContext: context.isSwitchingMemoryContext,
            activeProjectId: context.activeProjectId,
            itemProjectContextId: item.projectContextId
        ), let key = sessions.documentSessionKey(for: item) else {
            feedback.errorMessage = "Wait for the Memory context switch to finish before syncing this document."
            return
        }
        if sessions.projectOrgSelectionMutatingIds.contains(key.projectId) {
            feedback.errorMessage = "Wait for the Project memory selection to finish updating before syncing this document."
            return
        }
        let behindDraft = item.draft.flatMap { $0.freshness == .behind ? $0 : nil }
        let hasStaleResource = item.resource.map { resource in
            self.catalog.staleResourceSnapshots[resource.id]?.projectId == key.projectId
        } ?? false
        guard behindDraft != nil || hasStaleResource,
              sessions.synchronizingDocumentSessions.insert(key).inserted else { return }

        let generation = UUID()
        sessions.documentSynchronizationGenerations[key] = generation
        if behindDraft != nil {
            openDocumentForSync(item)
        }
        sessions.documentSynchronizationTasks[key] = Task { @MainActor [weak self] in
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
        let existingMode = navigation.tabs.first {
            $0.section == self.navigation.selectedSection
                && $0.projectId == tabProjectId
                && $0.itemId == item.id
        }?.mode
        // The command is consumed by a DocumentSession. Keep an existing
        // Source/Diff mode, and use Source for a newly opened document so Sync
        // never silently switches the user to the default Preview.
        navigation.open(item, mode: existingMode ?? .source)
    }

    private func syncStaleResource(
        _ item: MemoryListItem,
        key: MemoryDocumentSessionKey,
        generation: UUID
    ) async {
        guard let resourceId = item.resource?.id else { return }
        guard sessions.isCurrentDocumentSynchronization(key, generation: generation) else { return }
        do {
            // A debounce save may not have materialized its draft yet. Flush
            // before applying a remote deletion/update so that local text can
            // enter reconciliation instead of becoming an invisible orphan.
            try await edits.flushDocumentSave(key)
            guard sessions.isCurrentDocumentSynchronization(key, generation: generation) else { return }
            if let draft = edits.currentDraft(for: item) {
                openDocumentForSync(item)
                await prepareBehindDraftSync(
                    key: key,
                    draft: draft,
                    generation: generation
                )
                return
            }
        } catch is CancellationError {
            sessions.endDocumentSynchronization(key, generation: generation)
            return
        } catch {
            guard sessions.isCurrentDocumentSynchronization(key, generation: generation) else { return }
            sessions.endDocumentSynchronization(key, generation: generation)
            feedback.errorMessage = error.localizedDescription
            return
        }

        // A repeated click after the first task completed is already satisfied.
        guard let snapshot = catalog.staleResourceSnapshots[resourceId],
              snapshot.projectId == key.projectId else {
            sessions.endDocumentSynchronization(key, generation: generation)
            return
        }
        guard context.projects.first(where: { $0.id == snapshot.projectId })?.refCommitId
            == snapshot.observedProjectRefCommitId,
              context.projects.first(where: { $0.id == snapshot.projectId })?.selectedOrgResourceIds
            == snapshot.observedSelectedOrgResourceIds,
              context.projects.first(where: { $0.id == snapshot.projectId })?.orgSelectionRevision
            == snapshot.observedOrgSelectionRevision else {
            sessions.endDocumentSynchronization(key, generation: generation)
            feedback.errorMessage = DocumentSyncError.checkoutNoLongerCurrent.localizedDescription
            return
        }
        if let remote = snapshot.remote {
            catalog.staleResourceRefreshGenerations[snapshot.projectId] = UUID()
            if let index = catalog.resources.firstIndex(where: { $0.id == resourceId }) {
                catalog.resources[index] = remote
            } else {
                catalog.resources.append(remote)
            }
            catalog.provisionalStaleAdditionIds.remove(resourceId)
            navigation.refreshDocumentTabs(for: resourceId)
        } else {
            catalog.staleResourceRefreshGenerations[snapshot.projectId] = UUID()
            catalog.resources.removeAll { $0.id == resourceId }
            catalog.provisionalStaleAdditionIds.remove(resourceId)
            if let tab = navigation.tabs.first(where: {
                $0.itemId == resourceId && $0.projectId == key.projectId
            }) {
                navigation.closeTab(tab)
            }
        }
        catalog.staleResourceSnapshots.removeValue(forKey: resourceId)
        catalog.refreshVisibleStaleResourceIds()
        catalog.bumpDocumentContentGeneration(for: resourceId)
        catalog.advanceProjectRefIfPlanCompleted(
            projectId: snapshot.projectId,
            authoritativeCommitId: snapshot.authoritativeCommitId,
            authoritativeRefEtag: snapshot.authoritativeRefEtag,
            selectedOrgResourceIds: snapshot.selectedOrgResourceIds,
            orgSelectionRevision: snapshot.orgSelectionRevision
        )
        sessions.endDocumentSynchronization(key, generation: generation)
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
                let candidate = try await reconciliation.reconciliationCandidate(for: draft)
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
           let snapshot = catalog.staleResourceSnapshot(for: item) {
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

    private func prepareBehindDraftSync(
        key: MemoryDocumentSessionKey,
        draft: LocalDraft,
        generation: UUID
    ) async {
        guard sessions.isCurrentDocumentSynchronization(key, generation: generation) else { return }
        do {
            // Keep the editor locked across the single upload barrier and the
            // candidate POST so a late keystroke cannot be omitted.
            let synchronized = try await reconciliation.synchronizedDraftForReconciliation(
                itemId: key.itemId,
                draft: draft
            )
            guard sessions.isCurrentDocumentSynchronization(key, generation: generation) else {
                return
            }
            edits.installSynchronizedDraft(synchronized)
            guard synchronized.freshness == .behind else {
                // A provisional remote addition already uses the authoritative
                // generation as its draft base. It needs adoption, not a
                // reconciliation candidate for an already-current draft.
                if let resourceId = synchronized.targetId {
                    catalog.adoptCurrentStaleResource(resourceId)
                }
                sessions.endDocumentSynchronization(key, generation: generation)
                return
            }
            let candidate = try await reconciliation.requestReconciliationCandidate(for: synchronized)
            guard sessions.isCurrentDocumentSynchronization(key, generation: generation),
                  navigation.tabs.contains(where: {
                      $0.itemId == key.itemId && $0.projectId == key.projectId
                  }) else {
                sessions.endDocumentSynchronization(key, generation: generation)
                return
            }
            sessions.pendingDocumentReconciliationCandidatesBySession[key] = candidate
            sessions.documentReconciliationResolutions[key] = candidate.proposedState
                ?? candidate.draftState
        } catch is CancellationError {
            sessions.endDocumentSynchronization(key, generation: generation)
            return
        } catch {
            guard sessions.isCurrentDocumentSynchronization(key, generation: generation) else { return }
            sessions.endDocumentSynchronization(key, generation: generation)
            feedback.errorMessage = error.localizedDescription
        }
    }

    func uniqueDefaultPath(
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
            let carrierProjectId = projectId ?? context.activeProjectId
            scopedDrafts = edits.drafts.filter {
                $0.scope == .org && $0.projectId == carrierProjectId
            }
        } else if scope == .project, let activeProjectId = context.activeProjectId {
            scopedResources = MemoryTreeProjection.memoryTreeResources(
                catalog.resources,
                activeProjectId: activeProjectId,
                selectedOrgResourceIds: context.activeProject?.selectedOrgResourceIds ?? []
            )
            scopedDrafts = MemoryTreeProjection.memoryTreeDrafts(edits.drafts, activeProjectId: activeProjectId)
        } else {
            scopedResources = catalog.resources.filter { $0.scope == scope }
            scopedDrafts = edits.drafts.filter { $0.scope == scope }
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
}

enum DocumentPathChangeSource: String, Equatable, Sendable {
    case draft
    case shared
    case draftAndShared
}

struct DocumentPathChange: Equatable, Sendable {
    let source: DocumentPathChangeSource
    let from: String?
    let to: String?
}

struct DocumentDiffResult: Equatable, Sendable {
    let presentation: UnifiedDiffPresentation?
    let pathChanges: [DocumentPathChange]
}
