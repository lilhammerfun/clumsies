import Combine
import Foundation

@MainActor
final class ProjectRepositoriesModel: ObservableObject {
    private let context: WorkspaceContext
    private let projects: ProjectService
    private let fetchBindings: (String) async throws -> [DaemonProjectBinding]
    private let removeRepository: (DaemonProjectBinding) async throws -> Void
    private var generation = UUID()
    private var loadedProjectId: String?
    private var loadedAuthority: UUID?
    private var mutatingProjectId: String?
    private var mutationId = UUID()
    @Published private(set) var bindings: [DaemonProjectBinding] = []
    @Published private var isRefreshing = false
    @Published private(set) var isMutating = false
    var isLoading: Bool { isRefreshing || isMutating }
    @Published private(set) var errorMessage: String?

    init(context: WorkspaceContext, projects: ProjectService,
         fetchBindings: ((String) async throws -> [DaemonProjectBinding])? = nil,
         removeRepository: ((DaemonProjectBinding) async throws -> Void)? = nil) {
        self.context = context
        self.projects = projects
        self.fetchBindings = fetchBindings ?? { try await projects.projectBindings($0) }
        self.removeRepository = removeRepository ?? { try await projects.removeProjectRepository($0) }
    }

    func load() async {
        if loadedProjectId != context.activeProjectId || loadedAuthority != context.authorityGeneration {
            bindings = []
            errorMessage = nil
        }
        guard !isMutating || context.activeProjectId != mutatingProjectId else {
            ClientDiagnostics.record("repository_bindings_refresh_skipped", ["reason": "mutation_in_progress"])
            return
        }
        let request = UUID()
        generation = request
        loadedAuthority = context.authorityGeneration
        loadedProjectId = context.activeProjectId
        errorMessage = nil
        isRefreshing = false
        guard let projectId = context.activeProjectId else { return }
        let authority = context.authorityGeneration
        isRefreshing = true
        defer { if generation == request { isRefreshing = false } }
        do {
            let loaded = try await fetchBindings(projectId)
            ClientDiagnostics.record("repository_bindings_loaded", [
                "request_id": ClientDiagnostics.requestID ?? request.uuidString.lowercased(),
                "project_id": ClientDiagnostics.identifier(projectId),
                "binding_count": String(loaded.count),
                "result_current": String(generation == request && context.authorityGeneration == authority && context.activeProjectId == projectId),
                "workspace_ids": loaded.map { ClientDiagnostics.workspaceID($0.workspaceRoot) }.joined(separator: ",")
            ])
            try context.ensureAuthority(authority)
            guard generation == request, context.activeProjectId == projectId else { return }
            bindings = loaded
        } catch {
            guard generation == request, context.authorityGeneration == authority,
                  context.activeProjectId == projectId, !Task.isCancelled else { return }
            errorMessage = bindings.isEmpty ? error.actionMessage : error.backgroundMessage
        }
    }

    func remove(_ binding: DaemonProjectBinding) async {
        await mutate(projectId: binding.projectId, action: "remove", fields: [
            "workspace_id": ClientDiagnostics.workspaceID(binding.workspaceRoot), "revision": String(binding.revision)
        ]) {
            try await self.removeRepository(binding)
        }
    }

    func add(_ urls: [URL], projectId: String) async {
        await mutate(projectId: projectId, action: "add", fields: [
            "workspace_ids": urls.map { ClientDiagnostics.workspaceID($0.path) }.joined(separator: ",")
        ]) {
            _ = try await self.projects.addProjectRepositories(urls.map(\.path), projectId: projectId)
        }
    }

    private func mutate(projectId: String, action: String, fields: [String: String], operation: () async throws -> Void) async {
        let id = "req_" + UUID().uuidString.lowercased()
        await ClientDiagnostics.$requestID.withValue(id) {
            await performMutation(projectId: projectId, action: action, fields: fields, operation: operation)
        }
    }

    private func performMutation(projectId: String, action: String, fields: [String: String], operation: () async throws -> Void) async {
        var fields = fields
        fields["request_id"] = ClientDiagnostics.requestID
        fields["project_id"] = ClientDiagnostics.identifier(projectId)
        fields["action"] = action
        ClientDiagnostics.record("repository_mutation_requested", fields)
        guard context.activeProjectId == projectId, !isMutating else {
            fields["reason"] = context.activeProjectId != projectId ? "project_changed" : "busy"
            ClientDiagnostics.record("repository_mutation_skipped", fields)
            return
        }
        let authority = context.authorityGeneration
        let request = UUID()
        mutationId = request
        generation = request
        isRefreshing = false
        isMutating = true
        mutatingProjectId = projectId
        errorMessage = nil
        ClientDiagnostics.record("repository_mutation_started", fields)
        defer {
            if mutationId == request { isMutating = false; mutatingProjectId = nil }
        }
        do {
            try await operation()
            ClientDiagnostics.record("repository_mutation_persisted", fields)
            try context.ensureAuthority(authority)
            guard context.activeProjectId == projectId else {
                ClientDiagnostics.record("repository_mutation_result_ignored", fields.merging(["reason": "project_changed"]) { _, new in new })
                return
            }
            isMutating = false
            mutatingProjectId = nil
            await load()
            ClientDiagnostics.record("repository_mutation_refresh_finished", fields)
        } catch {
            fields.merge(ClientDiagnostics.failureFields(error)) { _, new in new }
            ClientDiagnostics.record("repository_mutation_failed", fields)
            guard generation == request, context.authorityGeneration == authority,
                  context.activeProjectId == projectId, !Task.isCancelled else {
                ClientDiagnostics.record("repository_mutation_error_ignored", fields)
                return
            }
            errorMessage = error.actionMessage
        }
    }
}
