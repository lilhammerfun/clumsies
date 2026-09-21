import Combine
import Foundation

@MainActor
final class MemoryGuidelinesModel: ObservableObject {
    private let workspaceContext: WorkspaceContext
    private let memoryModel: MemoryModel
    private var generation = UUID()

    init(context: WorkspaceContext, memory: MemoryModel) {
        workspaceContext = context
        memoryModel = memory
    }

    @Published private(set) var setup: MemoryGuidelinesSetup?
    @Published var error: String?
    @Published private(set) var isLoading = true
    @Published private(set) var isAdopting = false
    @Published private(set) var destinationChanged = false

    func canAdopt(_ setup: MemoryGuidelinesSetup) -> Bool {
        guard setup.projectId == workspaceContext.activeProjectId, !workspaceContext.isSwitchingMemoryContext else { return false }
        if case .useOrganization = setup.action { return workspaceContext.canManageProject(setup.projectId) }
        return true
    }

    func prepare() async {
        let requestGeneration = UUID()
        generation = requestGeneration
        guard let projectId = workspaceContext.activeProjectId else {
            setup = nil
            isLoading = false
            return
        }
        let authority = workspaceContext.authorityGeneration
        isLoading = true
        error = nil
        setup = nil
        destinationChanged = false
        do {
            let result = try await memoryModel.prepareMemoryGuidelines(projectId: projectId)
            try Task.checkCancellation()
            try workspaceContext.ensureAuthority(authority)
            guard generation == requestGeneration, workspaceContext.activeProjectId == projectId else { return }
            setup = result
            isLoading = false
        } catch where error.isUserCancellation {
            guard generation == requestGeneration, workspaceContext.authorityGeneration == authority, workspaceContext.activeProjectId == projectId, !Task.isCancelled else { return }
            isLoading = false
        } catch {
            guard generation == requestGeneration, workspaceContext.authorityGeneration == authority, workspaceContext.activeProjectId == projectId, !Task.isCancelled else { return }
            self.error = error.actionMessage
            isLoading = false
        }
    }

    func adopt() async {
        guard let setup, canAdopt(setup), !isAdopting else { return }
        let authority = workspaceContext.authorityGeneration
        let requestGeneration = generation
        isAdopting = true
        defer { isAdopting = false }
        do {
            let result = try await memoryModel.useMemoryGuidelines(setup)
            try workspaceContext.ensureAuthority(authority)
            guard generation == requestGeneration, workspaceContext.activeProjectId == setup.projectId else { return }
            destinationChanged = !result.hasSameDestination(as: setup)
            self.setup = result
        } catch where error.isUserCancellation {
            return
        } catch {
            guard generation == requestGeneration, workspaceContext.authorityGeneration == authority, workspaceContext.activeProjectId == setup.projectId else { return }
            self.error = error.actionMessage
        }
    }

}
