import Combine
import Foundation

@MainActor
final class ProjectService: ObservableObject {
    private let agents: AgentIntegrationService
    private let bundles: BundleStore
    private let catalog: MemoryCatalog
    private let context: WorkspaceContext
    private let edits: DraftStore
    private let refresh: DaemonSyncService
    private let sessions: DocumentSessions

    init(agents: AgentIntegrationService, bundles: BundleStore, catalog: MemoryCatalog, context: WorkspaceContext, edits: DraftStore, refresh: DaemonSyncService, sessions: DocumentSessions) {
        self.agents = agents
        self.bundles = bundles
        self.catalog = catalog
        self.context = context
        self.edits = edits
        self.refresh = refresh
        self.sessions = sessions
    }

    @Published var projectBindingsGeneration = UUID()

    private let projectOrgSelectionMutationGate = AsyncMutex()

    func createProject(
        name: String,
        description: String,
        idempotencyKey: String,
        repositoryPaths: [String],
        bundleId: String?
    ) async throws -> String {
        guard context.canCreateProject else { throw AdministrationError.forbidden }
        guard context.phase == .ready else { throw AdministrationError.unavailable }
        let generation = try context.beginAdministrationMutation()
        defer { self.context.finishAdministrationMutation(generation) }
        let repositoryPaths = normalizedRepositoryPaths(repositoryPaths)
        let created: ProjectRecord = try await context.server.send(
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
        try context.ensureCurrentAdministrationMutation(generation)
        context.projectRoles[created.id] = .admin
        let initialSelection = try await initializeProjectMemory(
            projectId: created.id,
            bundleId: bundleId,
            generation: generation
        )
        try context.ensureCurrentAdministrationMutation(generation)
        let project = ProjectState(
            id: created.id,
            name: created.name,
            refCommitId: nil,
            refEtag: "",
            selectedOrgResourceIds: projectOrgResourceIds(initialSelection),
            orgSelectionRevision: initialSelection.revision,
            isLoaded: false
        )
        if let index = context.projects.firstIndex(where: { $0.id == created.id }) {
            context.projects[index] = project
        } else {
            context.projects.insert(project, at: 0)
        }
        for repositoryPath in repositoryPaths {
            _ = try await context.daemon.replaceProjectBinding(
                .init(
                    workspaceRoot: repositoryPath,
                    projectId: created.id,
                    expectedRevision: nil
                )
            )
            try context.ensureCurrentAdministrationMutation(generation)
            projectBindingsGeneration = UUID()
        }
        context.projectDirectoryChanges.send()
        return created.id
    }

    func projectBindings(_ projectId: String) async throws -> [DaemonProjectBinding] {
        try await context.daemon.projectBindings(projectId)
    }

    func addProjectRepositories(
        _ repositoryPaths: [String],
        projectId: String
    ) async throws -> [DaemonProjectBinding] {
        var bindings: [DaemonProjectBinding] = []
        for repositoryPath in normalizedRepositoryPaths(repositoryPaths) {
            bindings.append(try await context.daemon.replaceProjectBinding(
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
        let adapters = try await agents.projectAgentAdapters(binding.projectId)
        for adapter in adapters where adapter.workspaceRoot == binding.workspaceRoot {
            _ = try await self.context.daemon.removeProjectAgentAdapter(
                .init(
                    workspaceRoot: binding.workspaceRoot,
                    adapter: adapter.adapter,
                    expectedRevision: adapter.revision
                )
            )
            didMutate = true
        }
        _ = try await context.daemon.removeProjectBinding(
            .init(
                workspaceRoot: binding.workspaceRoot,
                expectedRevision: binding.revision
            )
        )
        didMutate = true
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
        guard context.canManageProject(projectId) else {
            throw ServerClientError.forbidden("Project administrator access is required to manage project memory.")
        }
        guard !sessions.hasDocumentSynchronization(in: projectId) else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        guard context.projects.contains(where: { $0.id == projectId }) else {
            throw ProjectMemorySelectionError.projectUnavailable
        }
        try catalog.validateOrgResourceIds(resourceIds)
        try await withProjectOrgSelectionMutation {
            guard self.context.canManageProject(projectId) else {
                throw ServerClientError.forbidden(
                    "Project administrator access is required to manage project memory."
                )
            }
            guard self.context.projects.contains(where: { $0.id == projectId }) else {
                throw ProjectMemorySelectionError.projectUnavailable
            }
            try self.catalog.validateOrgResourceIds(resourceIds)
            guard !self.sessions.hasDocumentSynchronization(in: projectId),
                  self.sessions.projectOrgSelectionMutatingIds.insert(projectId).inserted else {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            defer { self.sessions.projectOrgSelectionMutatingIds.remove(projectId) }
            // Removing a selected Org resource can make its Project tab and
            // tree row disappear. Materialize every dirty editor in that
            // Project first so a failed save remains visible and recoverable
            // as a LocalDraft instead of being stranded in a debounce buffer.
            let pendingSaveKeys = self.edits.pendingDocumentSaves.compactMap { key, pending in
                let pendingIds = Set(
                    [pending.item.id, pending.item.draft?.id, pending.item.draft?.targetId]
                        .compactMap { $0 }
                )
                return key.projectId == projectId && !pendingIds.isDisjoint(with: resourceIds)
                    ? key
                    : nil
            }
            for key in pendingSaveKeys {
                try await self.edits.flushDocumentSave(key)
            }
            if case .remove = mutation,
               MemoryTreeProjection.hasActiveDraft(
                   in: projectId,
                   targetingAny: resourceIds,
                   drafts: self.edits.drafts
               ) {
                throw ProjectMemorySelectionError.activeDrafts
            }
            let current: ProjectOrgSelection = try await self.context.server.get(
                "/api/v1/projects/\(projectId)/org-selections"
            )
            let currentIds = self.projectOrgResourceIds(current)
            let nextIds = mutation.applying(resourceIds, to: currentIds)
            guard nextIds != currentIds else {
                // The Server may already reflect the requested state while a
                // stale local snapshot does not. Treat the authoritative GET
                // as a successful repair instead of leaving the UI behind.
                await self.applyProjectOrgSelection(current, toProject: projectId)
                return
            }
            guard self.context.canManageProject(projectId) else {
                throw ServerClientError.forbidden(
                    "Project administrator access is required to manage project memory."
                )
            }
            guard self.context.projects.contains(where: { $0.id == projectId }) else {
                throw ProjectMemorySelectionError.projectUnavailable
            }
            guard !self.sessions.hasDocumentSynchronization(in: projectId) else {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            let selection = try await replaceProjectOrgSelection(
                projectId: projectId,
                expectedRevision: current.revision,
                resourceIds: nextIds
            )
            await self.applyProjectOrgSelection(selection, toProject: projectId)
        }
    }

    private func applyProjectOrgSelection(
        _ selection: ProjectOrgSelection,
        toProject projectId: String
    ) async {
        let commit: (value: CommitStateResponse, response: DaemonServerResponse)? = try? await context.server.getWithMetadata(
            "/api/v1/projects/\(projectId)/commit-state"
        )
        let freshCommit = commit.flatMap { $0.response.isStaleCache ? nil : $0 }
        guard let index = context.projects.firstIndex(where: { $0.id == projectId }) else { return }
        let project = context.projects[index]
        guard selection.revision >= project.orgSelectionRevision else { return }
        catalog.clearStaleResourceState(for: projectId)
        context.projects[index] = ProjectState(
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
        catalog.documentsChanged.send()
        if projectId == context.activeProjectId {
            _ = await refresh.retrySync(channel: "commits", projectId: projectId)
        }
    }

    private func replaceProjectOrgSelection(
        projectId: String,
        expectedRevision: Int,
        resourceIds: Set<String>
    ) async throws -> ProjectOrgSelection {
        let request = ReplaceProjectOrgSelectionRequest(resourceIds: resourceIds.sorted())
        return try await context.server.send(
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
        let current: ProjectOrgSelection = try await context.server.get(
            "/api/v1/projects/\(projectId)/org-selections"
        )
        try context.ensureCurrentAdministrationMutation(generation)
        guard let bundleId else { return current }
        guard let bundle = bundles.bundles.first(where: { $0.id == bundleId }) else {
            throw ProjectSetupError.bundleNotFound
        }
        let resourceIds = Set(bundle.resourceIds)
        do {
            try catalog.validateOrgResourceIds(resourceIds)
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

    private func projectOrgResourceIds(_ selection: ProjectOrgSelection) -> Set<String> {
        Set(selection.memories.map(\.memoryId))
    }

    func withProjectOrgSelectionMutation<T>(
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
}
