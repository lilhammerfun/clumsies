import Combine
import Foundation

@MainActor
final class AgentIntegrationService: ObservableObject {
    private let context: WorkspaceContext
    private let feedback: WorkspaceFeedback

    init(context: WorkspaceContext, feedback: WorkspaceFeedback) {
        self.context = context
        self.feedback = feedback
    }

    @Published var legacyAgentAdapterConflicts: [DaemonLegacyAgentAdapterConflict] = []
    @Published var legacyAgentAdapterInspectionWarning: String?

    private var legacyAgentAdapterInspectionTask: Task<Void, Never>?

    func projectAgentAdapters(_ projectId: String) async throws -> [DaemonProjectAgentAdapter] {
        try await context.daemon.projectAgentAdapters(projectId)
    }

    func allProjectAgentAdapters() async throws -> [DaemonProjectAgentAdapter] {
        try await context.daemon.allProjectAgentAdapters()
    }

    func codexPluginStatus() async throws -> DaemonCodexPluginStatus {
        try await context.daemon.inspectCodexPlugin(
            .init(
                runtimeBinaryPath: try bundledAgentRuntimePath(),
                hostBinaryPath: try? WorkspaceLoader.installedCodexHostBinaryPath()
            )
        )
    }

    func setAgentAdapter(_ adapter: ProjectAgentAdapterKind, enabled: Bool) async throws -> [DaemonAgentAdapterSetting] {
        try await context.daemon.setAgentAdapter(.init(
            adapter: adapter,
            enabled: enabled,
            runtimeBinaryPath: try bundledAgentRuntimePath(),
            hostBinaryPath: adapter == .codex ? try? WorkspaceLoader.installedCodexHostBinaryPath() : nil
        ))
    }

    func applyLocalAgentAdapterResult(_ result: LocalAgentAdapterReconciliationResult) {
        let nextErrorMessage = Self.errorMessageAfterUpdatingLocalAgentAdapters(
            currentErrorMessage: feedback.errorMessage,
            previous: .init(
                conflicts: legacyAgentAdapterConflicts,
                inspectionWarning: legacyAgentAdapterInspectionWarning
            ),
            next: result
        )
        legacyAgentAdapterConflicts = result.conflicts
        legacyAgentAdapterInspectionWarning = result.inspectionWarning
        feedback.errorMessage = nextErrorMessage
    }

    nonisolated static func errorMessageAfterUpdatingLocalAgentAdapters(
        currentErrorMessage: String?,
        previous: LocalAgentAdapterReconciliationResult,
        next: LocalAgentAdapterReconciliationResult
    ) -> String? {
        let previousWarning = localAgentAdapterWarning(previous)
        guard currentErrorMessage == nil || currentErrorMessage == previousWarning else {
            return currentErrorMessage
        }
        return localAgentAdapterWarning(next)
    }

    nonisolated static func localAgentAdapterWarning(
        _ result: LocalAgentAdapterReconciliationResult
    ) -> String? {
        var messages: [String] = []
        if let inspectionWarning = result.inspectionWarning {
            messages.append(inspectionWarning)
        }
        let visible = result.conflicts.prefix(3).map { conflict in
            let adapter = conflict.adapter == .claudeCode ? "Claude Code" : conflict.adapter.rawValue
            return "\(adapter) \(conflict.scope) integration at \(conflict.targetRoot): \(conflict.message)"
        }
        messages.append(contentsOf: visible)
        if result.conflicts.count > visible.count {
            messages.append("\(result.conflicts.count - visible.count) more legacy integrations need review.")
        }
        return messages.isEmpty ? nil : messages.joined(separator: "\n")
    }

    nonisolated static func combinedAgentAdapterWarning(
        _ managedWarning: String?,
        _ legacyWarning: String?
    ) -> String? {
        let warnings = [managedWarning, legacyWarning].compactMap { $0 }
        return warnings.isEmpty ? nil : warnings.joined(separator: "\n")
    }

    func bundledAgentRuntimePath() throws -> String {
        try AppBundleRuntimeLocation.requireStable(Bundle.main.bundleURL)
        guard let path = Bundle.main.resourceURL?.appending(path: "clumsiesd").path,
              FileManager.default.isExecutableFile(atPath: path) else {
            throw ProjectSetupError.bundledAgentRuntimeMissing
        }
        return path
    }

    func startLoading(generation: UUID) {
        cancelLoading()
        let loader = context.loader
        legacyAgentAdapterInspectionTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.context.workspaceReloadGeneration == generation {
                    self.legacyAgentAdapterInspectionTask = nil
                }
            }
            let result = await loader.inspectLegacyAgentAdapters()
            guard let self,
                  context.workspaceReloadGeneration == generation,
                  context.phase == .ready,
                  !Task.isCancelled else {
                return
            }
            applyLocalAgentAdapterResult(.init(
                conflicts: result.conflicts,
                inspectionWarning: AgentIntegrationService.combinedAgentAdapterWarning(
                    legacyAgentAdapterInspectionWarning,
                    result.inspectionWarning
                )
            ))
        }
    }

    func cancelLoading() {
        legacyAgentAdapterInspectionTask?.cancel()
        legacyAgentAdapterInspectionTask = nil
    }
}

struct LocalAgentAdapterReconciliationResult: Equatable, Sendable {
    let conflicts: [DaemonLegacyAgentAdapterConflict]
    let inspectionWarning: String?
}
