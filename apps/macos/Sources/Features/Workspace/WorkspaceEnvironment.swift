import SwiftUI

struct WorkspaceActions {
    var reload: @MainActor () async -> Void = {}
    var selectProject: @MainActor (String) async -> Void = { _ in }
    var prepareIndex: @MainActor (Bool) async -> Void = { _ in }
    var reveal: @MainActor (MemoryListItem) async -> Void = { _ in }
}

private struct WorkspaceActionsKey: EnvironmentKey {
    static let defaultValue = WorkspaceActions()
}

extension EnvironmentValues {
    var workspaceActions: WorkspaceActions {
        get { self[WorkspaceActionsKey.self] }
        set { self[WorkspaceActionsKey.self] = newValue }
    }
}

extension View {
    func workspaceEnvironment(_ workspace: WorkspaceCoordinator) -> some View {
        self
            .environment(\.workspaceActions, WorkspaceActions(
                reload: { [weak workspace] in await workspace?.reload() },
                selectProject: { [weak workspace] in await workspace?.selectProject($0) },
                prepareIndex: { [weak workspace] in await workspace?.prepareWorkspaceIndex(includeContent: $0) },
                reveal: { [weak workspace] in await workspace?.reveal($0) }
            ))
            .environmentObject(workspace.context)
            .environmentObject(workspace.catalog)
            .environmentObject(workspace.edits)
            .environmentObject(workspace.sessions)
            .environmentObject(workspace.sync)
            .environmentObject(workspace.reconciliation)
            .environmentObject(workspace.bundles)
            .environmentObject(workspace.bundleSelection)
            .environmentObject(workspace.reviews)
            .environmentObject(workspace.projects)
            .environmentObject(workspace.agents)
            .environmentObject(workspace.refresh)
            .environmentObject(workspace.feedback)
            .environmentObject(workspace.inbox)
            .environmentObject(workspace.navigation)
            .environmentObject(workspace.memory)
    }
}
