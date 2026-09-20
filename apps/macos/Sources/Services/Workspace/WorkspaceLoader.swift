import AppKit
import CryptoKit
import Foundation

struct WorkspaceLoader: Sendable {
    let daemon: DaemonXPCClient
    let bootstrap: DaemonBootstrapController
    let server: ServerClient

    func load(
        onLocalAgentAdapters: @MainActor @Sendable (LocalAgentAdapterReconciliationResult) async
            -> Void = { _ in }
    ) async throws -> WorkspaceSnapshot {
        server.resetDataSource()
        let health = try await ensureDaemon()
        let (config, me, localAgentAdapters, currentUserWasStale) =
            try await Self.loadAuthenticatedWorkspaceIdentity(
            reconcileManagedAgentAdapters: {
                try await reconcileManagedAgentAdapters()
            },
            projectConfig: {
                try await daemon.projectConfig()
            },
            currentUser: {
                let result: (value: CurrentUserResponse, response: DaemonServerResponse) =
                    try await server.getWithMetadata("/api/v1/me")
                return (result.value, result.response.isStaleCache)
            },
            onManagedAgentAdapters: { result in
                await onLocalAgentAdapters(result)
            }
        )
        let activeProjectId = configuredProject(config, me: me)
        if let activeProjectId, config.projectId != activeProjectId {
            _ = try await daemon.selectProject(activeProjectId)
        }

        async let orgCommitRequest: (value: CommitStateResponse, response: DaemonServerResponse) = server.getWithMetadata(
            "/api/v1/org/commit-state"
        )
        let projectStateLoad: (
            states: [ProjectState],
            hasStaleServerResponse: Bool
        )
        if let activeProjectId {
            projectStateLoad = try await loadProjectStates(
                me.projects,
                activeProjectId: activeProjectId
            )
        } else {
            projectStateLoad = ([], false)
        }
        let projectStates = projectStateLoad.states
        let orgCommit = try await orgCommitRequest

        var scopes = [
            ResourceLoadScope(projectId: nil, projectName: nil, refCommitId: orgCommit.value.ref.commitId),
        ]
        if let activeProjectId,
           let activeProject = projectStates.first(where: { $0.id == activeProjectId }) {
            scopes.append(ResourceLoadScope(
                projectId: activeProject.id,
                projectName: activeProject.name,
                refCommitId: activeProject.refCommitId
            ))
        }
        let resourceGroups = try await concurrentMap(scopes, maxConcurrent: 2) { scope in
            try await loadResourcesWithMetadata(
                projectId: scope.projectId,
                projectName: scope.projectName,
                refCommitId: scope.refCommitId
            )
        }
        let resources = resourceGroups.flatMap { $0.resources }

        let verifiedOrgCommit: (value: CommitStateResponse, response: DaemonServerResponse) =
            try await server.getWithMetadata("/api/v1/org/commit-state")
        guard verifiedOrgCommit.value.ref.commitId == orgCommit.value.ref.commitId else {
            throw WorkspaceLoadError.sharedStateChangedDuringLoad
        }
        var verifiedProjectWasStale = false
        if let activeProjectId,
           let initialProject = projectStates.first(where: { $0.id == activeProjectId }),
           let reference = me.projects.first(where: { $0.projectId == activeProjectId }) {
            let verifiedProject = try await loadProjectStateWithMetadata(reference)
            verifiedProjectWasStale = verifiedProject.hasStaleServerResponse
            guard verifiedProject.state.refCommitId == initialProject.refCommitId,
                  verifiedProject.state.selectedOrgResourceIds == initialProject.selectedOrgResourceIds,
                  verifiedProject.state.orgSelectionRevision == initialProject.orgSelectionRevision else {
                throw WorkspaceLoadError.sharedStateChangedDuringLoad
            }
        }
        let hasStaleServerResponse = currentUserWasStale
            || orgCommit.response.isStaleCache
            || projectStateLoad.hasStaleServerResponse
            || resourceGroups.contains { $0.hasStaleServerResponse }
            || verifiedOrgCommit.response.isStaleCache
            || verifiedProjectWasStale
        return .init(
            account: me.user,
            organization: me.org,
            capabilities: Set(me.capabilities),
            projects: projectStates,
            projectRoles: Dictionary(uniqueKeysWithValues: me.projects.compactMap { project in
                project.role.map { (project.id, $0) }
            }),
            activeProjectId: activeProjectId,
            orgRefCommitId: orgCommit.value.ref.commitId,
            orgRefEtag: etag(from: orgCommit.response),
            resources: resources,
            runtime: .init(
                health: health,
                sync: nil,
                serverDataSource: hasStaleServerResponse ? "stale" : "live"
            ),
            legacyAgentAdapterConflicts: localAgentAdapters.conflicts,
            legacyAgentAdapterInspectionWarning: localAgentAdapters.inspectionWarning
        )
    }

    static func listAllDraftSummaries(
        listDrafts: @escaping @Sendable (DaemonDraftListQuery) async throws -> DaemonDraftListResponse
    ) async throws -> [DaemonDraftSummary] {
        var items: [DaemonDraftSummary] = []
        var cursor: String?

        while true {
            let page = try await listDrafts(.init(cursor: cursor, limit: 500))
            try Task.checkCancellation()
            items.append(contentsOf: page.items)
            guard let nextCursor = page.nextCursor else {
                return items
            }
            guard !page.items.isEmpty, nextCursor != cursor else {
                throw DaemonXPCError.invalidReply
            }
            cursor = nextCursor
        }
    }

    func loadDeferredDrafts(
        resources: [MemoryResource],
        accessibleProjectIds: Set<String>
    ) async throws -> (
        loadedBaselines: [MemoryResource],
        drafts: [LocalDraft],
        hasStaleServerResponse: Bool
    ) {
        try await Self.loadDeferredDrafts(
            resources: resources,
            accessibleProjectIds: accessibleProjectIds,
            listDrafts: { query in
                try await daemon.listDrafts(query)
            },
            loadDraft: { draftId in
                try await daemon.draft(draftId)
            },
            loadContent: { resource in
                try await loadContentWithMetadata(
                    for: resource,
                    allowingStaleCache: true
                )
            }
        )
    }

    static func loadDeferredDrafts(
        resources: [MemoryResource],
        accessibleProjectIds: Set<String>,
        listDrafts: @escaping @Sendable (DaemonDraftListQuery) async throws -> DaemonDraftListResponse,
        loadDraft: @escaping @Sendable (String) async throws -> DaemonDraftDetail,
        loadContent: @escaping @Sendable (MemoryResource) async throws
            -> (resource: MemoryResource, hasStaleServerResponse: Bool)
    ) async throws -> (
        loadedBaselines: [MemoryResource],
        drafts: [LocalDraft],
        hasStaleServerResponse: Bool
    ) {
        let draftSummaries = try await listAllDraftSummaries(listDrafts: listDrafts)
        try Task.checkCancellation()
        let activeDrafts = draftSummaries.filter {
            $0.status != .discarded
                && $0.status != .merged
                && accessibleProjectIds.contains($0.projectId)
        }
        let targetIds = Set(activeDrafts.compactMap(\.targetId))
        let baselines = resources.filter { targetIds.contains($0.id) && !$0.contentLoaded }
        let loadedBaselineResults = try await concurrentMap(
            baselines,
            transform: loadContent
        )
        try Task.checkCancellation()

        let loadedBaselines = loadedBaselineResults.map(\.resource)
        var hydratedResources = resources
        for loaded in loadedBaselines {
            if let index = hydratedResources.firstIndex(where: { $0.id == loaded.id }) {
                hydratedResources[index] = loaded
            }
        }
        let resourceSnapshot = hydratedResources
        let drafts = try await concurrentMap(activeDrafts) { summary in
            Self.mapDraft(try await loadDraft(summary.draftId), resources: resourceSnapshot)
        }
        try Task.checkCancellation()
        return (
            loadedBaselines,
            drafts,
            loadedBaselineResults.contains { $0.hasStaleServerResponse }
        )
    }

    func loadProject(id: String, name: String) async throws -> (state: ProjectState, resources: [MemoryResource]) {
        let loaded = try await loadProjectWithMetadata(id: id, name: name)
        return (loaded.state, loaded.resources)
    }

    func loadProjectWithMetadata(
        id: String,
        name: String
    ) async throws -> (
        state: ProjectState,
        resources: [MemoryResource],
        hasStaleServerResponse: Bool
    ) {
        let state = try await loadProjectStateWithMetadata(.init(projectId: id, name: name))
        let resources = try await loadResourcesWithMetadata(
            projectId: state.state.id,
            projectName: state.state.name,
            refCommitId: state.state.refCommitId
        )
        let verifiedState = try await loadProjectStateWithMetadata(
            .init(projectId: id, name: name)
        )
        guard state.state.refCommitId == verifiedState.state.refCommitId,
              state.state.selectedOrgResourceIds == verifiedState.state.selectedOrgResourceIds,
              state.state.orgSelectionRevision == verifiedState.state.orgSelectionRevision else {
            throw WorkspaceLoadError.sharedStateChangedDuringLoad
        }
        return (
            state.state,
            resources.resources,
            state.hasStaleServerResponse
                || resources.hasStaleServerResponse
                || verifiedState.hasStaleServerResponse
        )
    }

    func loadCachedProject(
        id: String,
        name: String
    ) async throws -> (state: ProjectState, resources: [MemoryResource])? {
        let checkout = try await daemon.projectCheckout(id)
        guard checkout.ready else { return nil }
        return Self.mapProjectCheckout(checkout, projectName: name)
    }

    static func mapProjectCheckout(
        _ checkout: DaemonProjectCheckout,
        projectName: String
    ) -> (state: ProjectState, resources: [MemoryResource]) {
        let resources = checkout.resources.compactMap { resource -> MemoryResource? in
            guard resource.scope == .project else { return nil }
            let kind = MemoryKind(resource.resourceKind)
            var document = EditableMemoryDocument(
                title: title(from: resource.path),
                path: resource.path,
                body: ""
            )
            document = apply(content: resource.content, to: document)
            return .init(
                id: resource.resourceId,
                scope: .project,
                projectId: checkout.projectId,
                projectName: projectName,
                kind: kind,
                contentHash: resource.contentHash,
                updatedAt: checkout.commitCreatedAt ?? "",
                refCommitId: checkout.commitId,
                contentLoaded: true,
                document: document
            )
        }
        return (
            .init(
                id: checkout.projectId,
                name: projectName,
                refCommitId: checkout.commitId,
                refEtag: checkout.refEtag ?? "",
                selectedOrgResourceIds: Set(checkout.selectedOrgResourceIds),
                orgSelectionRevision: checkout.orgSelectionRevision,
                isLoaded: true
            ),
            resources
        )
    }

    func loadContent(
        for resource: MemoryResource,
        allowingStaleCache: Bool = false
    ) async throws -> MemoryResource {
        let loaded = try await loadContentWithMetadata(
            for: resource,
            allowingStaleCache: allowingStaleCache
        )
        return loaded.resource
    }

    func loadContentWithMetadata(
        for resource: MemoryResource,
        allowingStaleCache: Bool = false
    ) async throws -> (resource: MemoryResource, hasStaleServerResponse: Bool) {
        guard !resource.contentLoaded else { return (resource, false) }
        let prefix = resource.projectId.map { "/api/v1/projects/\($0)" } ?? "/api/v1/org"
        var loaded = resource
        let result: (value: MemoryDetail, response: DaemonServerResponse) =
            try await server.getWithMetadata("\(prefix)/memories/\(resource.id)")
        loaded.document.body = try Self.validatedMemoryContent(
            for: resource,
            detail: result.value,
            response: result.response,
            allowingStaleCache: allowingStaleCache
        )
        loaded.contentLoaded = true
        return (loaded, result.response.isStaleCache)
    }

    nonisolated static func validatedMemoryContent(
        for resource: MemoryResource,
        detail: MemoryDetail,
        response: DaemonServerResponse,
        allowingStaleCache: Bool = false
    ) throws -> String {
        guard allowingStaleCache || !response.isStaleCache else {
            throw ServerClientError.invalidResponse(
                "A stale cached memory body cannot be attached to the current shared version."
            )
        }
        let digest = SHA256.hash(data: Data(detail.content.utf8))
        let actualContentHash = "sha256:" + digest.map { String(format: "%02x", $0) }.joined()
        guard detail.memory.memoryId == resource.id,
              detail.memory.contentHash == resource.contentHash,
              detail.memory.path == resource.document.path,
              actualContentHash == resource.contentHash else {
            throw ServerClientError.invalidResponse(
                "The memory body no longer matches the requested shared version."
            )
        }
        return detail.content
    }

    private func ensureDaemon() async throws -> DaemonHealth {
        let readiness = DaemonStartupReadiness()
        if ProcessInfo.processInfo.environment["CLUMSIES_SKIP_DAEMON_BUILD"] != "1" {
            _ = try await bootstrap.ensureRunning()
        }

        do {
            return try await readiness.waitForHealth { timeout in
                try await daemon.health(timeout: timeout)
            }
        } catch is DaemonXPCError {
            let status = await bootstrap.status()
            var details: [String] = []
            if status.installed {
                details.append("installed: true")
            } else {
                details.append("installed: false")
            }
            if status.running {
                details.append("running: true")
            } else {
                details.append("running: false")
            }
            if let pid = status.pid {
                details.append("pid: \(pid)")
            }
            if let lastError = status.error, !lastError.isEmpty {
                details.append("error: \(lastError)")
            }
            let detailString = details.joined(separator: ", ")
            throw DaemonXPCError.connectionFailed(detail: detailString.isEmpty ? nil : detailString)
        }
    }

    /// Move every daemon-owned integration to the runtime embedded in the
    /// currently running App before authentication or Server access. Adapter
    /// files deliberately point at the App bundle, so an App update must
    /// reconcile existing installations even while the user is signed out or
    /// the Hub is unreachable.
    private func reconcileManagedAgentAdapters() async throws
        -> LocalAgentAdapterReconciliationResult {
        let runtimePath = try Self.bundledAgentRuntimePath()
        let codexHostPath = await MainActor.run { try? Self.installedCodexHostBinaryPath() }
        var warnings: [String] = []
        let settings = try await daemon.agentAdapterSettings()
        for setting in settings where setting.configured {
            do {
                _ = try await daemon.setAgentAdapter(.init(
                    adapter: setting.adapter,
                    enabled: setting.enabled,
                    runtimeBinaryPath: runtimePath,
                    hostBinaryPath: setting.adapter == .codex ? codexHostPath : nil
                ))
            } catch {
                warnings.append("\(setting.adapter.title): \(error.localizedDescription)")
            }
        }
        return .init(conflicts: [], inspectionWarning: warnings.isEmpty ? nil : warnings.joined(separator: "\n"))
    }

    func inspectLegacyAgentAdapters() async -> LocalAgentAdapterReconciliationResult {
        do {
            let runtimePath = try Self.bundledAgentRuntimePath()
            let inspection = try await daemon.inspectLegacyAgentAdapters(
                runtimeBinaryPath: runtimePath
            )
            return .init(conflicts: inspection.conflicts, inspectionWarning: nil)
        } catch {
            return .init(
                conflicts: [],
                inspectionWarning: Self.legacyAgentAdapterInspectionWarning(for: error)
            )
        }
    }

    static func legacyAgentAdapterInspectionWarning(for error: Error) -> String {
        if let daemonError = error as? DaemonXPCError,
           case .daemon(let payload) = daemonError,
           payload.code == "project_agent_adapter_invalid_runtime" {
            return "The resident daemon rejected the bundled Agent runtime. Archived integration "
                + "inspection was skipped. Reinstall and restart Clumsies so the App and daemon use "
                + "the same build. To replace the resident Debug installation, run "
                + "just install-macos; distributed Release "
                + "builds must use an accepted release signature."
        }
        return "Clumsies updated its managed integrations, but could not inspect the "
            + "archived Zig CLI integration store. Review any old global or repository "
            + "MCP and hook entries manually. \(error.localizedDescription)"
    }

    static func loadAuthenticatedWorkspaceIdentity(
        reconcileManagedAgentAdapters: () async throws
            -> LocalAgentAdapterReconciliationResult,
        projectConfig: () async throws -> DaemonProjectConfig,
        currentUser: () async throws -> (
            value: CurrentUserResponse,
            hasStaleServerResponse: Bool
        ),
        onManagedAgentAdapters: @MainActor @Sendable (LocalAgentAdapterReconciliationResult) async
            -> Void = { _ in }
    ) async throws -> (
        DaemonProjectConfig,
        CurrentUserResponse,
        LocalAgentAdapterReconciliationResult,
        Bool
    ) {
        let localAgentAdapters = try await reconcileManagedAgentAdapters()
        await onManagedAgentAdapters(localAgentAdapters)
        let config = try await projectConfig()
        guard config.hasAccessToken && config.hasRefreshToken else {
            throw WorkspaceLoadError.authenticationRequired
        }
        let currentUser = try await currentUser()
        return (
            config,
            currentUser.value,
            localAgentAdapters,
            currentUser.hasStaleServerResponse
        )
    }

    static func bundledAgentRuntimePath(
        bundle: Bundle = .main,
        fileManager: FileManager = .default
    ) throws -> String {
        try AppBundleRuntimeLocation.requireStable(bundle.bundleURL)
        guard let path = bundle.resourceURL?.appending(path: "clumsiesd").path,
              fileManager.isExecutableFile(atPath: path) else {
            throw ProjectSetupError.bundledAgentRuntimeMissing
        }
        return path
    }

    @MainActor
    static func installedCodexHostBinaryPath(
        workspace: NSWorkspace = .shared,
        fileManager: FileManager = .default
    ) throws -> String {
        try codexHostBinaryPath(
            applicationURL: workspace.urlForApplication(
                withBundleIdentifier: "com.openai.codex"
            ),
            fileManager: fileManager
        )
    }

    static func codexHostBinaryPath(
        applicationURL: URL?,
        fileManager: FileManager = .default
    ) throws -> String {
        guard let path = applicationURL?
            .appending(path: "Contents/Resources/codex")
            .path,
            fileManager.isExecutableFile(atPath: path)
        else {
            throw ProjectSetupError.codexHostMissing
        }
        return path
    }

    private func configuredProject(_ config: DaemonProjectConfig, me: CurrentUserResponse) -> String? {
        if let projectId = config.projectId, me.projects.contains(where: { $0.projectId == projectId }) {
            return projectId
        }
        return me.defaultProjectId ?? me.projects.first?.projectId
    }

    private func etag(from response: DaemonServerResponse) -> String {
        response.headers.first { $0.key.caseInsensitiveCompare("etag") == .orderedSame }?.value ?? ""
    }

    private func loadProjectStates(
        _ projects: [ProjectReference],
        activeProjectId: String
    ) async throws -> (states: [ProjectState], hasStaleServerResponse: Bool) {
        guard let active = projects.first(where: { $0.projectId == activeProjectId }) else {
            throw WorkspaceLoadError.noProjects
        }
        let loaded = try await loadProjectStateWithMetadata(active)
        return (
            projects.map { project in
                if project.projectId == loaded.state.id { return loaded.state }
                return .init(
                    id: project.projectId,
                    name: project.name,
                    refCommitId: nil,
                    refEtag: "",
                    selectedOrgResourceIds: [],
                    orgSelectionRevision: 0,
                    isLoaded: false
                )
            },
            loaded.hasStaleServerResponse
        )
    }

    private func loadProjectStateWithMetadata(
        _ project: ProjectReference
    ) async throws -> (state: ProjectState, hasStaleServerResponse: Bool) {
        async let commitRequest: (value: CommitStateResponse, response: DaemonServerResponse) = server.getWithMetadata(
            "/api/v1/projects/\(project.projectId)/commit-state"
        )
        async let selectionRequest: (value: ProjectOrgSelection, response: DaemonServerResponse) = server.getWithMetadata(
            "/api/v1/projects/\(project.projectId)/org-selections"
        )
        let (commit, selection) = try await (commitRequest, selectionRequest)
        return (
            .init(
                id: project.projectId,
                name: project.name,
                refCommitId: commit.value.ref.commitId,
                refEtag: etag(from: commit.response),
                selectedOrgResourceIds: Set(selection.value.memories.map(\.memoryId)),
                orgSelectionRevision: selection.value.revision,
                isLoaded: true
            ),
            commit.response.isStaleCache || selection.response.isStaleCache
        )
    }

    private func loadResources(
        projectId: String?,
        projectName: String?,
        refCommitId: String?
    ) async throws -> [MemoryResource] {
        try await loadResourcesWithMetadata(
            projectId: projectId,
            projectName: projectName,
            refCommitId: refCommitId
        ).resources
    }

    private func loadResourcesWithMetadata(
        projectId: String?,
        projectName: String?,
        refCommitId: String?
    ) async throws -> (resources: [MemoryResource], hasStaleServerResponse: Bool) {
        let prefix = projectId.map { "/api/v1/projects/\($0)" } ?? "/api/v1/org"
        let metadata: (items: [MemoryMetadata], hasStaleServerResponse: Bool) =
            try await loadAllWithMetadata("\(prefix)/memories")
        return (metadata.items.map { metadata in
            .init(
                id: metadata.memoryId,
                scope: projectId == nil ? .org : .project,
                projectId: projectId,
                projectName: projectName,
                kind: .init(.memory),
                contentHash: metadata.contentHash,
                updatedAt: metadata.updatedAt,
                refCommitId: refCommitId,
                contentLoaded: false,
                document: .init(
                    title: metadata.name,
                    path: metadata.path,
                    body: ""
                )
            )
        }, metadata.hasStaleServerResponse)
    }

    func loadBundles() async throws -> (
        records: [PersonalBundle],
        hasStaleServerResponse: Bool
    ) {
        let metadata: (
            items: [PersonalBundleMetadata],
            hasStaleServerResponse: Bool
        ) = try await loadAllWithMetadata("/api/v1/me/bundles")
        let bundles = try await concurrentMap(metadata.items) { item in
            let detail: (value: PersonalBundleDetail, response: DaemonServerResponse) =
                try await server.getWithMetadata("/api/v1/me/bundles/\(item.bundleId)")
            let bundle = PersonalBundle(
                id: item.bundleId,
                name: item.name,
                description: item.description,
                resourceIds: detail.value.memories.map(\.memoryId),
                revision: item.revision,
                updatedAt: item.updatedAt
            )
            return (
                record: bundle,
                hasStaleServerResponse: detail.response.isStaleCache
            )
        }
        return (
            bundles.map { $0.record },
            metadata.hasStaleServerResponse
                || bundles.contains { $0.hasStaleServerResponse }
        )
    }

    func loadReviews() async throws -> (
        records: [ReviewRecord],
        hasStaleServerResponse: Bool
    ) {
        let metadata: (
            items: [ReviewMetadata],
            hasStaleServerResponse: Bool
        ) = try await loadAllWithMetadata("/api/v1/reviews")
        return (
            metadata.items.map(Self.mapReview),
            metadata.hasStaleServerResponse
        )
    }

    private func loadAllWithMetadata<Item: Decodable & Sendable>(
        _ path: String
    ) async throws -> (items: [Item], hasStaleServerResponse: Bool) {
        var output: [Item] = []
        var cursor: String?
        var hasStaleServerResponse = false
        repeat {
            var query = [URLQueryItem(name: "limit", value: "200")]
            if let cursor { query.append(.init(name: "cursor", value: cursor)) }
            let page: (value: ListResponse<Item>, response: DaemonServerResponse) =
                try await server.getWithMetadata(path, query: query)
            try Task.checkCancellation()
            output += page.value.items
            hasStaleServerResponse = hasStaleServerResponse || page.response.isStaleCache
            cursor = page.value.pageInfo.hasMore ? page.value.pageInfo.nextCursor : nil
        } while cursor != nil
        try Task.checkCancellation()
        return (output, hasStaleServerResponse)
    }

    static func mapReview(_ detail: ReviewDetail) -> ReviewRecord {
        mapReview(detail.review)
    }

    static func mapReview(_ metadata: ReviewMetadata) -> ReviewRecord {
        return .init(
            id: metadata.reviewId,
            projectId: metadata.projectId,
            draftId: metadata.draftId,
            title: metadata.title,
            description: metadata.description,
            author: metadata.author,
            status: metadata.status,
            version: metadata.version,
            decisionBody: metadata.decisionBody,
            approvedResultHash: metadata.approvedResultHash,
            decidedBy: metadata.decidedBy,
            decidedAt: metadata.decidedAt,
            freshness: metadata.coordination.freshness,
            reconciliation: metadata.coordination.reconciliation,
            reconciliationCandidateId: metadata.coordination.candidateId,
            currentCommitId: metadata.coordination.currentCommitId,
            updatedAt: metadata.updatedAt,
            draftIds: metadata.draftIds ?? [metadata.draftId],
            autoRebased: metadata.coordination.autoRebased == true
        )
    }

    static func mapDraft(_ detail: DaemonDraftDetail, resources: [MemoryResource]) -> LocalDraft {
        let summary = detail.draft
        let base = summary.targetId.flatMap { id in
            resources.first { $0.id == id && $0.contentLoaded }
        }
        let hasSelfContainedContent = detail.operations.contains { operation in
            switch operation.operation {
            case .create, .update: true
            case .rename, .delete, .discard: false
            }
        }
        var document = base?.document ?? .init(
            title: title(from: summary.path ?? "Untitled"),
            path: summary.path ?? "untitled.md",
            body: ""
        )
        var deletion = false
        for operation in detail.operations {
            switch operation.operation {
            case .create(let path, let content, _):
                document.path = path
                document = apply(content: content, to: document)
                deletion = false
            case .update(_, let content, _):
                document = apply(content: content, to: document)
                deletion = false
            case .rename(_, let newPath, _):
                document.path = newPath
            case .delete:
                deletion = true
            case .discard:
                break
            }
        }
        document.title = title(from: document.path)
        return .init(
            id: summary.draftId,
            projectId: summary.projectId,
            serverId: summary.serverDraftId,
            serverVersion: summary.serverVersion,
            baseCommitId: summary.baseCommitId,
            currentCommitId: summary.currentCommitId,
            freshness: summary.freshness,
            hasUpstreamResourceChanges: summary.hasUpstreamResourceChanges,
            reconciliation: summary.reconciliation,
            reconciliationCandidateId: summary.reconciliationCandidateId,
            scope: summary.scope == .org ? .org : .project,
            kind: .init(summary.resourceKind),
            targetId: summary.targetId,
            status: summary.status,
            origin: detail.operations.last?.source ?? .desktop,
            syncStatus: detail.operations.last?.syncStatus ?? .synced,
            updatedAt: summary.updatedAt,
            document: document,
            isDeletion: deletion,
            documentBaselineAvailable: base != nil
                || summary.targetId == nil
                || hasSelfContainedContent
        )
    }

    private static func apply(
        content: DaemonDraftContent,
        to document: EditableMemoryDocument
    ) -> EditableMemoryDocument {
        var document = document
        document.body = content.content
        return document
    }

    private static func title(from path: String) -> String {
        URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent
    }

    private func title(from path: String) -> String {
        Self.title(from: path)
    }
}

private struct ResourceLoadScope: Sendable {
    let projectId: String?
    let projectName: String?
    let refCommitId: String?
}
