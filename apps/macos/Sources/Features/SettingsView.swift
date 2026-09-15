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
    var onCompleted: (() -> Void)?
    @State private var settings: [DaemonAgentAdapterSetting] = []
    @State private var selected: Set<ProjectAgentAdapterKind> = [.codex]
    @State private var codexStatus: DaemonCodexPluginStatus?
    @State private var isWorking = false
    @State private var hasLoaded = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            if onCompleted != nil {
                VStack(spacing: 10) {
                    BrandLogoView(size: 52)
                    Text("Connect Your Agents")
                        .font(.system(size: 22, weight: .semibold))
                    Text("Choose the agents you use on this Mac. You can change this later in Settings.")
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .padding(.horizontal, 36)
                .padding(.top, 36)
            }
            Form {
                Section {
                    ForEach(ProjectAgentAdapterKind.allCases) { adapter in
                        VStack(alignment: .leading, spacing: 5) {
                            Toggle(adapter.title, isOn: Binding(
                                get: { selected.contains(adapter) },
                                set: { enabled in
                                    if enabled { selected.insert(adapter) }
                                    else { selected.remove(adapter) }
                                    if onCompleted == nil {
                                        Task { await save(adapter, enabled: enabled) }
                                    }
                                }
                            ))
                            .disabled(isWorking || !hasLoaded)
                            if adapter == .codex {
                                Text(codexDescription)
                                    .font(.caption).foregroundStyle(.secondary)
                            } else if onCompleted == nil,
                                      let setting = settings.first(where: { $0.adapter == adapter }),
                                      setting.enabled {
                                Text(setting.installed
                                    ? (adapter == .dsh ? "Runtime configured; profile bridge required" : "Installed for this Mac")
                                    : "Ready to install")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            if let setting = settings.first(where: { $0.adapter == adapter }),
                               setting.configured, setting.legacyRepositories > 0 {
                                Text("\(setting.legacyRepositories) old repository configuration(s) still need cleanup. Reconnect missing folders and retry.")
                                    .font(.caption).foregroundStyle(.orange)
                            }
                        }
                    }
                } header: {
                    Text("Agents on This Mac")
                } footer: {
                    Text("Install once for all projects. The repository binding selects which project’s Memory each agent uses.")
                }

                if onCompleted == nil {
                    Section {
                        Button("Repair Selected Integrations") { Task { await saveSelection() } }
                            .disabled(isWorking || !hasLoaded)
                    }
                }
                Section {
                    Text("After changing Codex, restart it and start a new task, then review Clumsies in /hooks.")
                    if selected.contains(.dsh) {
                        Text("dsh also needs its MCP and lifecycle bridge registered in your dsh profile.")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)

                if let errorMessage {
                    Section {
                        Text(errorMessage).foregroundStyle(.red).textSelection(.enabled)
                        Button("Retry") { Task { await load() } }.disabled(isWorking)
                    }
                }
            }
            .formStyle(.grouped)
            .scrollContentBackground(onCompleted == nil ? .visible : .hidden)
            .font(.system(size: 13))
            .toggleStyle(.switch)

            if onCompleted != nil {
                HStack {
                    Button("Set Up Later") { onCompleted?() }
                        .disabled(isWorking)
                    if isWorking { ProgressView().controlSize(.small) }
                    Spacer()
                    Button(selected.isEmpty ? "Continue Without Adapters" : "Install and Continue") {
                        Task { await saveSelection() }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(isWorking || !hasLoaded)
                }
                .padding(24)
            }
        }
        .task { await load() }
    }

    private var codexDescription: String {
        if !selected.contains(.codex) { return "Disabled" }
        guard let codexStatus else { return "Selected by default" }
        if !codexStatus.hostInstalled { return "Will install when Codex is available" }
        if codexStatus.ready { return "Plugin installed and enabled" }
        return codexStatus.pluginInstalled ? "Plugin needs repair" : "Plugin not installed"
    }

    private func load() async {
        isWorking = true
        defer { isWorking = false }
        do {
            let loaded = try await store.daemon.agentAdapterSettings()
            try Task.checkCancellation()
            settings = loaded
            selected = Set(loaded.filter(\.enabled).map(\.adapter))
            hasLoaded = true
            errorMessage = nil
            codexStatus = try await store.codexPluginStatus()
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func save(_ adapter: ProjectAgentAdapterKind, enabled: Bool) async {
        isWorking = true
        defer { isWorking = false }
        do {
            settings = try await store.setAgentAdapter(adapter, enabled: enabled)
            selected = Set(settings.filter(\.enabled).map(\.adapter))
            errorMessage = nil
            if adapter == .codex { codexStatus = try await store.codexPluginStatus() }
        } catch {
            if let actual = try? await store.daemon.agentAdapterSettings() {
                settings = actual
                selected = Set(actual.filter(\.enabled).map(\.adapter))
            }
            errorMessage = error.localizedDescription
        }
    }

    private func saveSelection() async {
        isWorking = true
        defer { isWorking = false }
        do {
            for adapter in ProjectAgentAdapterKind.allCases {
                try Task.checkCancellation()
                settings = try await store.setAgentAdapter(adapter, enabled: selected.contains(adapter))
            }
            errorMessage = nil
            if let onCompleted { onCompleted() }
            else { codexStatus = try await store.codexPluginStatus() }
        } catch {
            errorMessage = error.localizedDescription
        }
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
