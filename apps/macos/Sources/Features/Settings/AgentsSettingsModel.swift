import Combine
import Foundation

@MainActor
final class AgentsSettingsModel: ObservableObject {
    private let workspaceContext: WorkspaceContext
    private let agentIntegration: AgentIntegrationService

    init(context: WorkspaceContext, integration: AgentIntegrationService) {
        workspaceContext = context
        agentIntegration = integration
    }

    @Published private(set) var settings: [DaemonAgentAdapterSetting] = []
    @Published var selected: Set<ProjectAgentAdapterKind> = [.codex]
    @Published private(set) var codexStatus: DaemonCodexPluginStatus?
    @Published private(set) var isWorking = false
    @Published private(set) var hasLoaded = false
    @Published private(set) var errorMessage: String?

    var codexDescription: String {
        if !selected.contains(.codex) { return "Disabled" }
        guard let codexStatus else { return "Selected by default" }
        if !codexStatus.hostInstalled { return "Will install when Codex is available" }
        if codexStatus.ready { return "Plugin installed and enabled" }
        return codexStatus.pluginInstalled ? "Plugin needs repair" : "Plugin not installed"
    }

    func load() async {
        guard !isWorking else { return }
        isWorking = true
        defer { isWorking = false }
        do {
            let loaded = try await workspaceContext.daemon.agentAdapterSettings()
            try Task.checkCancellation()
            settings = loaded
            selected = Set(loaded.filter(\.enabled).map(\.adapter))
            hasLoaded = true
            errorMessage = nil
            codexStatus = try await agentIntegration.codexPluginStatus()
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func save(_ adapter: ProjectAgentAdapterKind, enabled: Bool) async {
        guard !isWorking else { return }
        isWorking = true
        defer { isWorking = false }
        do {
            settings = try await agentIntegration.setAgentAdapter(adapter, enabled: enabled)
            selected = Set(settings.filter(\.enabled).map(\.adapter))
            errorMessage = nil
            if adapter == .codex { codexStatus = try await agentIntegration.codexPluginStatus() }
        } catch {
            if let actual = try? await workspaceContext.daemon.agentAdapterSettings() {
                settings = actual
                selected = Set(actual.filter(\.enabled).map(\.adapter))
            }
            errorMessage = error.localizedDescription
        }
    }

    func saveSelection(refreshStatus: Bool) async -> Bool {
        guard !isWorking else { return false }
        isWorking = true
        defer { isWorking = false }
        do {
            for adapter in ProjectAgentAdapterKind.allCases {
                try Task.checkCancellation()
                settings = try await agentIntegration.setAgentAdapter(adapter, enabled: selected.contains(adapter))
            }
            errorMessage = nil
            if refreshStatus { codexStatus = try await agentIntegration.codexPluginStatus() }
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }
}
