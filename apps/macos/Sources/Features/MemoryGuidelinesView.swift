import MarkdownUI
import SwiftUI

struct MemoryGuidelinesSetupView: View {
    @ObservedObject var store: WorkspaceStore
    @State private var setup: MemoryGuidelinesSetup?
    @State private var error: String?
    @State private var isLoading = true
    @State private var isAdopting = false
    @State private var preview: GuidelinesPreviewContent?
    @State private var destinationChanged = false

    var body: some View {
        Group {
            if isLoading {
                ContentLoadingView(title: "Checking Memory Guidelines…")
            } else if let error {
                ContentUnavailableView {
                    Label("Memory Guidelines Unavailable", systemImage: "doc.badge.ellipsis")
                } description: {
                    Text(error)
                } actions: {
                    Button("Try Again") { Task { await prepare() } }
                }
            } else if let setup {
                ContentUnavailableView {
                    Label(title(for: setup), systemImage: "doc.text")
                } description: {
                    Text(description(for: setup))
                        .frame(maxWidth: 440)
                } actions: {
                    VStack(spacing: 12) {
                        Button(actionTitle(for: setup)) { Task { await adopt() } }
                            .buttonStyle(.borderedProminent)
                            .keyboardShortcut(.defaultAction)
                            .disabled(!canAdopt(setup) || isAdopting)
                        if setup.action == .createDefault {
                            Button("Preview guidelines and their sources") {
                                do {
                                    preview = GuidelinesPreviewContent(
                                        documents: try MemoryGuidelines.defaultDocuments(occupiedPaths: setup.occupiedPaths)
                                    )
                                } catch {
                                    self.error = error.localizedDescription
                                }
                            }
                            .buttonStyle(.link)
                            .disabled(isAdopting)
                            Text("Creates CLUMSIES.md and starter folders for knowledge, procedures, and lessons.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        if case .useOrganization = setup.action, !canAdopt(setup) {
                            Text("Ask a project administrator to add your organization's guidelines.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        if destinationChanged {
                            Text("Your memory guidelines changed. Review the current option above.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        if isAdopting {
                            ProgressView().controlSize(.small)
                        }
                    }
                }
            }
        }
        .task(id: store.activeProjectId) { await prepare() }
        .sheet(item: $preview) { content in
            MemoryGuidelinesPreview(documents: content.documents) {
                preview = nil
                Task { await adopt() }
            }
        }
    }

    private func canAdopt(_ setup: MemoryGuidelinesSetup) -> Bool {
        guard setup.projectId == store.activeProjectId, !store.isSwitchingMemoryContext else { return false }
        if case .useOrganization = setup.action { return store.canManageProject(setup.projectId) }
        return true
    }

    private func title(for setup: MemoryGuidelinesSetup) -> String {
        switch setup.action {
        case .createDefault: "Give your memory a starting point"
        case .useOrganization: "Use your team's memory guidelines"
        case .open: "Your memory guidelines are ready"
        }
    }

    private func description(for setup: MemoryGuidelinesSetup) -> String {
        switch setup.action {
        case .createDefault:
            "Memory guidelines tell agents what to remember and how to keep it useful. Start with our defaults and make them your own."
        case .useOrganization:
            "Your organization already has memory guidelines at \(setup.path). Use them in this project."
        case .open:
            "Your guidelines at \(setup.path) tell agents how to organize, update, and retire knowledge. You can edit them at any time."
        }
    }

    private func actionTitle(for setup: MemoryGuidelinesSetup) -> String {
        switch setup.action {
        case .createDefault: "Set Up Guidelines"
        case .useOrganization: "Use Team Guidelines"
        case .open: "Open Guidelines"
        }
    }

    private func prepare() async {
        guard let projectId = store.activeProjectId else { return }
        isLoading = true
        error = nil
        setup = nil
        destinationChanged = false
        preview = nil
        do {
            let result = try await store.prepareMemoryGuidelines(projectId: projectId)
            try Task.checkCancellation()
            setup = result
            isLoading = false
        } catch is CancellationError {
            guard store.activeProjectId == projectId, !Task.isCancelled else { return }
            error = "The project changed while checking memory guidelines. Try again."
            isLoading = false
        } catch {
            guard store.activeProjectId == projectId, !Task.isCancelled else { return }
            self.error = error.localizedDescription
            isLoading = false
        }
    }

    private func adopt() async {
        guard let setup, canAdopt(setup), !isAdopting else { return }
        isAdopting = true
        defer { isAdopting = false }
        do {
            let result = try await store.useMemoryGuidelines(setup)
            destinationChanged = !result.hasSameDestination(as: setup)
            self.setup = result
        } catch is CancellationError {
            return
        } catch {
            guard store.activeProjectId == setup.projectId else { return }
            self.error = error.localizedDescription
        }
    }
}

private struct GuidelinesPreviewContent: Identifiable {
    let id = UUID()
    let documents: [EditableMemoryDocument]
}

private struct MemoryGuidelinesPreview: View {
    let documents: [EditableMemoryDocument]
    let onUse: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var selectedPath = MemoryGuidelines.defaultPath

    private var document: EditableMemoryDocument {
        documents.first { $0.path == selectedPath } ?? documents[0]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Memory Guidelines").font(.title2.bold())
            Text("Choose how agents maintain your memory. You can edit these guidelines at any time. App updates will preserve your changes.")
                .foregroundStyle(.secondary)
            Picker("Starter document", selection: $selectedPath) {
                ForEach(documents, id: \.path) { document in
                    Text(document.title).tag(document.path)
                }
            }
            .pickerStyle(.segmented)
            ScrollView {
                Markdown(document.body)
                    .markdownTheme(.gitHub)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
            }
            .background(.background)
            HStack {
                Text(document.path).font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Set Up Guidelines", action: onUse)
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding(24)
        .frame(width: 680, height: 640)
    }
}
