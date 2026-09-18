import Combine
import Foundation

@MainActor
final class ProjectRepositoriesModel: ObservableObject {
    private let context: WorkspaceContext
    private let projects: ProjectService
    private let fetchBindings: (String) async throws -> [DaemonProjectBinding]
    private var generation = UUID()
    @Published private(set) var bindings: [DaemonProjectBinding] = []
    @Published private(set) var isLoading = false
    @Published private(set) var errorMessage: String?

    init(context: WorkspaceContext, projects: ProjectService,
         fetchBindings: ((String) async throws -> [DaemonProjectBinding])? = nil) {
        self.context = context
        self.projects = projects
        self.fetchBindings = fetchBindings ?? { try await projects.projectBindings($0) }
    }

    func load() async {
        let request = UUID()
        generation = request
        bindings = []
        errorMessage = nil
        isLoading = false
        guard let projectId = context.activeProjectId else { return }
        let authority = context.authorityGeneration
        isLoading = true
        defer { if generation == request { isLoading = false } }
        do {
            let loaded = try await fetchBindings(projectId)
            try context.ensureAuthority(authority)
            guard generation == request, context.activeProjectId == projectId else { return }
            bindings = loaded
        } catch {
            guard generation == request, context.authorityGeneration == authority,
                  context.activeProjectId == projectId, !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }

    func remove(_ binding: DaemonProjectBinding) async {
        await mutate(projectId: binding.projectId) {
            try await self.projects.removeProjectRepository(binding)
        }
    }

    func add(_ urls: [URL], projectId: String) async {
        await mutate(projectId: projectId) {
            _ = try await self.projects.addProjectRepositories(urls.map(\.path), projectId: projectId)
        }
    }

    private func mutate(projectId: String, operation: () async throws -> Void) async {
        guard context.activeProjectId == projectId, !isLoading else { return }
        let authority = context.authorityGeneration
        let request = generation
        isLoading = true
        errorMessage = nil
        defer { if generation == request { isLoading = false } }
        do {
            try await operation()
            try context.ensureAuthority(authority)
            guard context.activeProjectId == projectId else { return }
            await load()
        } catch {
            guard generation == request, context.authorityGeneration == authority,
                  context.activeProjectId == projectId, !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }
}
