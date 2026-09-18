import SwiftUI

extension View {
    func workspaceEnvironment(_ workspace: WorkspaceCoordinator) -> some View {
        self
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
            .environmentObject(workspace.navigation)
            .environmentObject(workspace.memory)
    }
}
