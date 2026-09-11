import AppKit
import SwiftUI

struct GeneralSettingsView: View {
    @ObservedObject var softwareUpdateController: SoftwareUpdateController

    private var version: String {
        let short = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "Unknown"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        return build.isEmpty ? short : "\(short) (\(build))"
    }

    var body: some View {
        Form {
            Section {
                VStack(spacing: 10) {
                    SettingsIcon(symbol: "gearshape.fill", color: .gray, size: 52)
                    Text("General").font(.system(size: 22, weight: .semibold))
                    Text("App information and software updates.")
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 14)
                LabeledContent("Version", value: version)
                    .textSelection(.enabled)
            }
            Section("Updates") {
                Toggle(
                    "Automatically check for updates",
                    isOn: Binding(
                        get: { softwareUpdateController.automaticallyChecksForUpdates },
                        set: { softwareUpdateController.automaticallyChecksForUpdates = $0 }
                    )
                )
                Toggle(
                    "Automatically download updates",
                    isOn: Binding(
                        get: { softwareUpdateController.automaticallyDownloadsUpdates },
                        set: { softwareUpdateController.automaticallyDownloadsUpdates = $0 }
                    )
                )
                .disabled(!softwareUpdateController.allowsAutomaticUpdates)
                LabeledContent("Software updates") {
                    Button("Check for Updates…") { softwareUpdateController.checkForUpdates() }
                        .disabled(!softwareUpdateController.canCheckForUpdates)
                }
            }
        }
        .formStyle(.grouped)
        .font(.system(size: 13))
        .toggleStyle(.switch)
    }
}

struct AgentsSettingsView: View {
    @ObservedObject var store: WorkspaceStore
    @State private var status: DaemonCodexPluginStatus?
    @State private var isWorking = false
    @State private var errorMessage: String?
    @State private var repairMessage: String?

    var body: some View {
        Form {
            Section {
                LabeledContent("Status") {
                    if isWorking {
                        ProgressView().controlSize(.small)
                    } else if errorMessage != nil {
                        Text("Unavailable").foregroundStyle(.secondary)
                    } else if let status {
                        Label(
                            status.ready ? "Ready" : "Needs repair",
                            systemImage: status.ready ? "checkmark.circle.fill" : "exclamationmark.circle.fill"
                        )
                        .foregroundStyle(status.ready ? Color.green : Color.orange)
                    } else {
                        Text("Checking…").foregroundStyle(.secondary)
                    }
                }
                LabeledContent("Version", value: status?.installedVersion ?? (status == nil ? "—" : "Not installed"))
                LabeledContent("Codex integration") {
                    Button("Repair") { Task { await repair() } }
                        .disabled(isWorking || status?.hostInstalled == false)
                }

                if let status {
                    DisclosureGroup("Installation details") {
                        LabeledContent("Host", value: status.hostInstalled ? "Installed" : "Not installed")
                        LabeledContent("Marketplace", value: marketplaceLabel(status))
                        LabeledContent("Plugin installed", value: status.pluginInstalled ? "Yes" : "No")
                        LabeledContent("Plugin enabled", value: status.pluginEnabled ? "Yes" : "No")
                        LabeledContent("Expected version", value: status.expectedVersion)
                    }
                }
                DisclosureGroup("After plugin changes") {
                    Text("Restart Codex and start a new task, then review Clumsies in /hooks. Plugin readiness does not confirm hook trust or AgentRun readiness.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let errorMessage {
                    Text(errorMessage)
                        .textSelection(.enabled)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let repairMessage {
                    Text(repairMessage)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } header: {
                Text("Codex")
            } footer: {
                Text("Clumsies maintains this integration automatically. Repository bindings determine which project’s Memory is used.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            RepositoryAgentSettingsView(store: store)
        }
        .formStyle(.grouped)
        .font(.system(size: 13))
        .toggleStyle(.switch)
        .task { await load() }
    }

    private func marketplaceLabel(_ status: DaemonCodexPluginStatus) -> String {
        if status.marketplaceConflict { return "Conflict" }
        return status.marketplaceInstalled ? "Installed" : "Not installed"
    }

    private func load() async {
        isWorking = true
        errorMessage = nil
        defer { isWorking = false }
        do {
            status = try await store.codexPluginStatus()
        } catch is CancellationError {
            return
        } catch {
            guard !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }

    private func repair() async {
        isWorking = true
        errorMessage = nil
        repairMessage = nil
        defer { isWorking = false }
        do {
            status = try await store.repairCodexPlugin()
            repairMessage = "Repair completed. Restart Codex and start a new task."
        } catch is CancellationError {
            return
        } catch {
            guard !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }
}

private struct AgentRepositoryProject: Identifiable {
    let id: String
    let name: String
    let repositories: [DaemonProjectBinding]
}

private struct RepositoryAgentSettingsView: View {
    @ObservedObject var store: WorkspaceStore
    @State private var projects: [AgentRepositoryProject] = []
    @State private var adapters: [DaemonProjectAgentAdapter] = []
    @State private var pendingValues: [String: Bool] = [:]
    @State private var workingKeys: Set<String> = []
    @State private var isLoading = false
    @State private var errorMessage: String?

    var body: some View {
        Group {
            if projects.isEmpty || errorMessage != nil {
                Section("Repository Integrations") {
                    if isLoading, projects.isEmpty {
                        LabeledContent("Repositories") {
                            ProgressView().controlSize(.small)
                        }
                    } else if store.projects.isEmpty {
                        Text("Create a project and bind a repository to configure these agents.")
                            .foregroundStyle(.secondary)
                    } else if projects.isEmpty {
                        Text("Add a repository in Project Settings to configure these agents.")
                            .foregroundStyle(.secondary)
                    }
                    if let errorMessage {
                        Text(errorMessage)
                            .textSelection(.enabled)
                            .foregroundStyle(.red)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }

            ForEach(projects) { project in
                ForEach(project.repositories) { repository in
                    Section {
                        ForEach(ProjectAgentAdapterKind.repositoryIntegrationCases) { adapter in
                            Toggle(
                                adapter.title,
                                isOn: adapterBinding(adapter, repository: repository)
                            )
                            .toggleStyle(.switch)
                            .disabled(!workingKeys.isEmpty)
                            .accessibilityLabel(
                                "\(adapter.title) for \(URL(fileURLWithPath: repository.workspaceRoot).lastPathComponent) in \(project.name)"
                            )
                        }
                    } header: {
                        Text("\(project.name) · \(URL(fileURLWithPath: repository.workspaceRoot).lastPathComponent)")
                    } footer: {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(repository.workspaceRoot)
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .help(repository.workspaceRoot)
                                .textSelection(.enabled)
                            Text("Changes update Clumsies-managed configuration in this repository.")
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                }
            }
        }
        .task(id: [store.projectBindingsGeneration.uuidString]
            + store.projects.map { "\($0.id):\($0.name)" }) {
            await load()
        }
    }

    private func load() async {
        isLoading = true
        errorMessage = nil
        defer {
            if !Task.isCancelled {
                isLoading = false
            }
        }
        do {
            try await reload()
        } catch is CancellationError {
            return
        } catch {
            guard !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }

    private func reload() async throws {
        var nextProjects: [AgentRepositoryProject] = []
        async let nextAdapters = store.allProjectAgentAdapters()
        for project in store.projects {
            try Task.checkCancellation()
            let repositories = try await store.projectBindings(project.id)
            if !repositories.isEmpty {
                nextProjects.append(.init(
                    id: project.id,
                    name: project.name,
                    repositories: repositories
                ))
            }
        }
        let loadedAdapters = try await nextAdapters
        try Task.checkCancellation()
        projects = nextProjects
        adapters = loadedAdapters
    }

    private func adapterBinding(
        _ adapter: ProjectAgentAdapterKind,
        repository: DaemonProjectBinding
    ) -> Binding<Bool> {
        let key = adapterKey(adapter, repository)
        return Binding(
            get: {
                pendingValues[key]
                    ?? (currentAdapter(adapter, repository) != nil)
            },
            set: { enabled in
                guard workingKeys.isEmpty else { return }
                pendingValues[key] = enabled
                workingKeys.insert(key)
                let current = currentAdapter(adapter, repository)
                Task {
                    do {
                        try await store.setProjectAgentAdapter(
                            adapter,
                            enabled: enabled,
                            projectId: repository.projectId,
                            workspaceRoot: repository.workspaceRoot,
                            current: current
                        )
                        try await reload()
                        errorMessage = nil
                    } catch {
                        let message = error.localizedDescription
                        try? await reload()
                        errorMessage = message
                    }
                    pendingValues.removeValue(forKey: key)
                    workingKeys.remove(key)
                }
            }
        )
    }

    private func currentAdapter(
        _ adapter: ProjectAgentAdapterKind,
        _ repository: DaemonProjectBinding
    ) -> DaemonProjectAgentAdapter? {
        adapters.first {
            $0.adapter == adapter
                && $0.serverUrl == repository.serverUrl
                && $0.projectId == repository.projectId
                && $0.workspaceRoot == repository.workspaceRoot
        }
    }

    private func adapterKey(
        _ adapter: ProjectAgentAdapterKind,
        _ repository: DaemonProjectBinding
    ) -> String {
        "\(repository.serverUrl):\(repository.workspaceRoot):\(adapter.rawValue)"
    }
}

struct SupportSettingsView: View {
    let onShowLogs: () -> Void

    var body: some View {
        Form {
            Section {
                LabeledContent("Logs") {
                    Button("Show in Finder", action: onShowLogs)
                }
                LabeledContent("Diagnostics") {
                    Button("Export…") { ClientDiagnostics.presentExport() }
                }
            } footer: {
                Text("Use logs to help investigate a problem with Clumsies.")
            }
        }
        .formStyle(.grouped)
        .font(.system(size: 13))
    }
}

struct ProjectMemoryCacheSettings: View {
    private enum Confirmation: String, Identifiable {
        case reset
        case clear

        var id: String { rawValue }
    }

    @ObservedObject var store: WorkspaceStore
    @State private var storage: DaemonProjectStorage?
    @State private var move: DaemonProjectStorageMove?
    @State private var isWorking = false
    @State private var errorMessage: String?
    @State private var confirmation: Confirmation?

    var body: some View {
        Section("Memory Cache") {
            if let projectId = store.activeProjectId {
                if let storage {
                    LabeledContent("Location") {
                        Text(storage.selectedRootPath)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(storage.selectedRootPath)
                    }
                    LabeledContent("Used", value: Self.byteCount.string(fromByteCount: Int64(storage.sizeBytes)))
                    LabeledContent("Status", value: availabilityLabel(storage.availability))

                    if let move, !move.state.isTerminal {
                        HStack(spacing: 8) {
                            ProgressView()
                                .controlSize(.small)
                            Text(moveLabel(move.state))
                                .foregroundStyle(.secondary)
                        }
                    }
                    if let diagnostic = storage.diagnostic {
                        Label(diagnostic, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.secondary)
                    }
                    if let errorMessage {
                        Text(errorMessage)
                            .textSelection(.enabled)
                            .foregroundStyle(.red)
                    }

                    HStack {
                        Button("Choose...") { Task { await chooseLocation(projectId: projectId) } }
                        Button("Reveal in Finder") { reveal(storage.managedRootPath) }
                            .disabled(storage.availability == .unavailable)
                        Button("Reset") { confirmation = .reset }
                            .disabled(storage.mode == .standard)
                        Button("Clear Cache...") { confirmation = .clear }
                    }
                    .disabled(isWorking || move?.state.isTerminal == false)
                } else if isWorking {
                    ProgressView()
                } else {
                    Text(errorMessage ?? "Storage status is unavailable.")
                        .textSelection(.enabled)
                        .foregroundStyle(.secondary)
                }
            } else {
                Text("Select a Project to configure its local storage.")
                    .foregroundStyle(.secondary)
            }
        }
        .task(id: store.activeProjectId) {
            await loadStorage()
        }
        .confirmationDialog(
            confirmation == .reset ? "Reset to Default Location?" : "Clear Project Cache?",
            isPresented: Binding(
                get: { confirmation != nil },
                set: { if !$0 { confirmation = nil } }
            ),
            titleVisibility: .visible
        ) {
            if confirmation == .reset {
                Button("Reset", role: .destructive) { Task { await resetLocation() } }
            } else if confirmation == .clear {
                Button("Clear Cache", role: .destructive) { Task { await clearCache() } }
            }
            Button("Cancel", role: .cancel) { confirmation = nil }
        } message: {
            if confirmation == .reset {
                Text("Clumsies will move the Project cache back to its standard macOS location.")
            } else {
                Text("Drafts and settings are preserved. Commit generations and the search index will be rebuilt.")
            }
        }
    }

    private func loadStorage() async {
        guard let projectId = store.activeProjectId else {
            storage = nil
            move = nil
            return
        }
        isWorking = true
        errorMessage = nil
        defer { isWorking = false }
        do {
            let loaded = try await store.daemon.projectStorage(projectId)
            storage = loaded
            if let moveId = loaded.activeMoveId {
                await monitorMove(moveId, projectId: projectId)
            } else {
                move = nil
            }
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func chooseLocation(projectId: String) async {
        guard let storage else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = true
        panel.prompt = "Choose"
        guard await panel.selectionResponse == .OK, let url = panel.url else { return }

        isWorking = true
        errorMessage = nil
        defer { isWorking = false }
        do {
            let bookmark = try url.bookmarkData(
                options: [],
                includingResourceValuesForKeys: nil,
                relativeTo: nil
            )
            let created = try await store.daemon.replaceProjectStorage(
                .init(
                    projectId: projectId,
                    selectedRootPath: url.path,
                    handoffBookmarkData: bookmark.base64EncodedString(),
                    expectedLocationRevision: storage.locationRevision
                )
            )
            move = created
            await monitorMove(created.moveId, projectId: projectId)
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func resetLocation() async {
        confirmation = nil
        guard let storage, let projectId = store.activeProjectId else { return }
        isWorking = true
        errorMessage = nil
        defer { isWorking = false }
        do {
            let created = try await store.daemon.resetProjectStorage(
                .init(projectId: projectId, expectedLocationRevision: storage.locationRevision)
            )
            move = created
            await monitorMove(created.moveId, projectId: projectId)
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func clearCache() async {
        confirmation = nil
        guard let storage, let projectId = store.activeProjectId else { return }
        isWorking = true
        errorMessage = nil
        defer { isWorking = false }
        do {
            self.storage = try await store.daemon.clearProjectCache(
                .init(projectId: projectId, expectedLocationRevision: storage.locationRevision)
            )
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func monitorMove(_ moveId: String, projectId: String) async {
        do {
            while !Task.isCancelled {
                let current = try await store.daemon.projectStorageMove(moveId)
                move = current
                if current.state.isTerminal {
                    if let moveError = current.errorMessage {
                        errorMessage = moveError
                    } else if current.state == .failed {
                        errorMessage = "The storage move failed."
                    }
                    storage = try await store.daemon.projectStorage(projectId)
                    return
                }
                try await Task.sleep(for: .milliseconds(500))
            }
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func reveal(_ path: String) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    private func availabilityLabel(_ availability: DaemonProjectStorageAvailability) -> String {
        switch availability {
        case .ready: "Ready"
        case .moving: "Moving"
        case .unavailable: "Unavailable"
        }
    }

    private func moveLabel(_ state: DaemonProjectStorageMoveState) -> String {
        switch state {
        case .preparing: "Preparing"
        case .materializing: "Copying cache"
        case .verifying: "Verifying"
        case .switching: "Switching location"
        case .cleaning: "Cleaning up"
        case .completed: "Completed"
        case .failed: "Failed"
        }
    }

    private static let byteCount: ByteCountFormatter = {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter
    }()
}

private extension NSOpenPanel {
    var selectionResponse: NSApplication.ModalResponse {
        get async {
            await withCheckedContinuation { continuation in
                begin { continuation.resume(returning: $0) }
            }
        }
    }
}
