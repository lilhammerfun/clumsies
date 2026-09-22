import AppKit
import SwiftUI
import UniformTypeIdentifiers

struct LocalProjectRecoveryView: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: InboxStore
    let retry: @MainActor () async -> SyncRetryOutcome
    @State private var isWorking = false
    @State private var error: String?
    @State private var task: Task<Void, Never>?

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 16) {
                Text("Unavailable Projects").font(.title2.bold())
                Text("Unavailable projects are paused. Removing a binding keeps the repository and local drafts.")
                    .foregroundStyle(.secondary)
                if store.unavailableProjects.isEmpty {
                    Text("All local projects are available.")
                } else {
                    List(store.unavailableProjects) { project in
                        VStack(alignment: .leading, spacing: 10) {
                            Text(project.name).font(.headline)
                            ForEach(project.bindings) { binding in
                                HStack {
                                    Text(binding.workspaceRoot).textSelection(.enabled)
                                        .lineLimit(2).truncationMode(.middle)
                                    Spacer()
                                    Button("Remove Binding") {
                                        run { try await store.removeUnavailableBinding(binding) }
                                    }
                                }
                            }
                            if project.draftCount > 0 {
                                Text("\(project.draftCount) local drafts retained").foregroundStyle(.secondary)
                                Button("Export Drafts…") { export(project) }
                            }
                        }.padding(.vertical, 8)
                    }.listStyle(.inset)
                }
                FormErrorMessage(message: error)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .padding(24)
            .disabled(isWorking)

            SheetActionBar(
                confirmationTitle: Text("Check Again"), cancellationTitle: "Close",
                isWorking: isWorking, confirmationIdentifier: "local-projects-retry",
                cancel: { dismiss() }, confirm: {
                    run {
                        if case .failed(let message) = await retry() {
                            throw ActionFailure(message)
                        }
                        await store.refresh()
                    }
                }
            )
        }
        .frame(width: 620, height: 420)
        .interactiveDismissDisabled(isWorking)
        .onDisappear { task?.cancel() }
    }

    private func run(_ operation: @escaping @MainActor () async throws -> Void) {
        isWorking = true
        error = nil
        task = Task { @MainActor in
            defer { isWorking = false }
            do { try await operation() }
            catch is CancellationError { }
            catch { self.error = error.actionMessage }
        }
    }

    private func export(_ project: DaemonUnavailableProject) {
        let panel = NSSavePanel()
        panel.title = String(localized: "Export Drafts…")
        panel.allowedContentTypes = [.json]
        panel.nameFieldStringValue = "\(project.projectId)-drafts.json"
        panel.begin { response in
            guard response == .OK, let url = panel.url else { return }
            run { try await store.exportUnavailableDrafts(project.projectId, to: url) }
        }
    }
}
