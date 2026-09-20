import AppKit
import SwiftUI

struct ProjectMemoryCacheSettings: View {
    private enum Confirmation: String, Identifiable {
        case reset
        case clear

        var id: String { rawValue }
    }

    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @StateObject private var model: ProjectStorageModel
    @State private var confirmation: Confirmation?

    init(model: @autoclosure @escaping () -> ProjectStorageModel) {
        _model = StateObject(wrappedValue: model())
    }

    private func chooseLocation(projectId: String) async {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = true
        panel.prompt = String(localized: "Choose")
        guard await panel.selectionResponse == .OK, let url = panel.url else { return }
        await model.chooseLocation(projectId: projectId, url: url)
    }

    var body: some View {
        Section("Memory Cache") {
            if let projectId = workspaceContext.activeProjectId {
                if let storage = model.storage {
                    LabeledContent("Location") {
                        Text(storage.selectedRootPath)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(storage.selectedRootPath)
                    }
                    LabeledContent("Used", value: Self.byteCount.string(fromByteCount: Int64(storage.sizeBytes)))
                    LabeledContent("Status", value: self.availabilityLabel(storage.availability))

                    if let move = model.move, !move.state.isTerminal {
                        HStack(spacing: 8) {
                            ProgressView()
                                .controlSize(.small)
                            Text(self.moveLabel(move.state))
                                .foregroundStyle(.secondary)
                        }
                    }
                    if let diagnostic = storage.diagnostic {
                        Label(diagnostic, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.secondary)
                    }
                    if let errorMessage = model.errorMessage {
                        Text(errorMessage)
                            .textSelection(.enabled)
                            .foregroundStyle(.red)
                    }

                    HStack {
                        Button("Choose...") { Task { await self.chooseLocation(projectId: projectId) } }
                        Button("Reveal in Finder") { self.reveal(storage.managedRootPath) }
                            .disabled(storage.availability == .unavailable)
                        Button("Reset") { self.confirmation = .reset }
                            .disabled(storage.mode == .standard)
                        Button("Clear Cache...") { self.confirmation = .clear }
                    }
                    .disabled(self.model.isWorking || model.move?.state.isTerminal == false)
                } else if self.model.isWorking {
                    ProgressView()
                } else {
                    Text(self.model.errorMessage ?? String(localized: "Storage status is unavailable."))
                        .textSelection(.enabled)
                        .foregroundStyle(.secondary)
                }
            } else {
                Text("Select a Project to configure its local storage.")
                    .foregroundStyle(.secondary)
            }
        }
        .task(id: workspaceContext.activeProjectId) {
            await self.model.loadStorage()
        }
        .onDisappear { model.cancel() }
        .confirmationDialog(
            confirmation == .reset ? "Reset to Default Location?" : "Clear Project Cache?",
            isPresented: Binding(
                get: { self.confirmation != nil },
                set: { if !$0 { self.confirmation = nil } }
            ),
            titleVisibility: .visible
        ) {
            if self.confirmation == .reset {
                Button("Reset", role: .destructive) { Task { self.confirmation = nil; await self.model.resetLocation() } }
            } else if self.confirmation == .clear {
                Button("Clear Cache", role: .destructive) { Task { self.confirmation = nil; await self.model.clearCache() } }
            }
            Button("Cancel", role: .cancel) { self.confirmation = nil }
        } message: {
            if self.confirmation == .reset {
                Text("Clumsies will move the Project cache back to its standard macOS location.")
            } else {
                Text("Drafts and settings are preserved. Commit generations and the search index will be rebuilt.")
            }
        }
    }

    private func reveal(_ path: String) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    private func availabilityLabel(_ availability: DaemonProjectStorageAvailability) -> String {
        switch availability {
        case .ready: String(localized: "Ready")
        case .moving: String(localized: "Moving")
        case .unavailable: String(localized: "Unavailable")
        }
    }

    private func moveLabel(_ state: DaemonProjectStorageMoveState) -> String {
        switch state {
        case .preparing: String(localized: "Preparing")
        case .materializing: String(localized: "Copying cache")
        case .verifying: String(localized: "Verifying")
        case .switching: String(localized: "Switching location")
        case .cleaning: String(localized: "Cleaning up")
        case .completed: String(localized: "Completed")
        case .failed: String(localized: "Failed")
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
