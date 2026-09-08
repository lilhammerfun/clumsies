import AppKit
import SwiftUI

enum ProjectMetadataValidation {
    static func isValid(name: String, description: String) -> Bool {
        let normalizedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedDescription = description.trimmingCharacters(in: .whitespacesAndNewlines)
        return !normalizedName.isEmpty
            && normalizedName.count <= 120
            && normalizedDescription.count <= 4_000
    }
}

struct ProjectCreationSheet: View {
    @ObservedObject var store: WorkspaceStore
    var onCreated: ((String) async -> Void)? = nil
    var onUnsavedChangesChange: (Bool) -> Void = { _ in }
    @Environment(\.dismiss) private var dismiss
    @FocusState private var nameFocused: Bool
    @State private var name = ""
    @State private var description = ""
    @State private var repositories: [URL] = []
    @State private var selectedBundleId: String?
    @State private var showsOptions = false
    @State private var isCreating = false
    @State private var errorMessage: String?
    @State private var idempotencyKey = UUID().uuidString.lowercased()

    var body: some View {
        VStack(spacing: 0) {
            Text("New Project")
                .font(.headline)
                .padding(.top, 20)
            Form {
                Section {
                    TextField("Name", text: $name).focused($nameFocused)
                    TextField("Description", text: $description, axis: .vertical)
                        .lineLimit(2...4)
                }
                Section {
                    DisclosureGroup("Additional options", isExpanded: $showsOptions) {
                        Picker("Initial memory", selection: $selectedBundleId) {
                            Text("None").tag(Optional<String>.none)
                            ForEach(store.bundles) { bundle in
                                Text(bundle.name).tag(Optional(bundle.id))
                            }
                        }
                        ForEach(repositories, id: \.path) { repository in
                            HStack {
                                Text(repository.lastPathComponent).lineLimit(1)
                                    .help(repository.path)
                                Spacer()
                                Button {
                                    repositories.removeAll { $0 == repository }
                                } label: { Image(systemName: "minus.circle") }
                                    .buttonStyle(.borderless)
                                    .accessibilityLabel("Remove \(repository.lastPathComponent)")
                            }
                        }
                        Button("Attach Repositories…") { chooseRepositories() }
                        Text("Repositories can also be attached later on this Mac.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
                if let errorMessage {
                    Text(errorMessage).foregroundStyle(.red).textSelection(.enabled)
                }
            }
            .formStyle(.grouped)
            .disabled(isCreating)
            HStack {
                if isCreating { ProgressView().controlSize(.small) }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction).disabled(isCreating)
                Button("Create") { create() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!ProjectMetadataValidation.isValid(name: name, description: description) || isCreating)
            }
            .padding(16)
        }
        .frame(width: 480, height: showsOptions ? 500 : 320)
        .interactiveDismissDisabled(isCreating)
        .onAppear { nameFocused = true }
        .onChange(of: name.isEmpty && description.isEmpty && repositories.isEmpty && selectedBundleId == nil) { _, empty in
            onUnsavedChangesChange(!empty)
        }
        .onDisappear { onUnsavedChangesChange(false) }
    }

    private func chooseRepositories() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.prompt = "Attach"
        panel.begin { response in
            guard response == .OK else { return }
            let existing = Set(repositories.map(\.standardized.path))
            repositories += panel.urls.map(\.standardized).filter { !existing.contains($0.path) }
            repositories.sort { $0.path < $1.path }
        }
    }

    private func create() {
        guard !isCreating else { return }
        isCreating = true
        errorMessage = nil
        Task {
            do {
                let id = try await store.createProject(
                    name: name, description: description, idempotencyKey: idempotencyKey,
                    repositoryPaths: repositories.map(\.path), bundleId: selectedBundleId
                )
                onUnsavedChangesChange(false)
                if let onCreated {
                    await onCreated(id)
                } else {
                    store.selectedSection = .memory
                    store.showsProjectSettings = false
                    await store.selectProject(id)
                }
                dismiss()
            } catch {
                errorMessage = error.localizedDescription
                isCreating = false
            }
        }
    }
}

struct ProjectUnavailableView: View {
    @ObservedObject var store: WorkspaceStore

    var body: some View {
        ContentUnavailableView {
            Label("No Projects", systemImage: "folder")
        } description: {
            if store.canManageProjects {
                Text("Create a Project to start organizing local memory.")
            } else {
                Text("Ask an organization administrator to grant you access to a Project.")
            }
        } actions: {
            if store.canManageProjects {
                Button("New Project…") {
                    store.presentProjectCreation()
                }
                .keyboardShortcut(.defaultAction)
            } else {
                Button("Refresh") {
                    Task { await store.reload() }
                }
            }
        }
    }
}

struct ProjectSettingsView: View {
    @ObservedObject var store: WorkspaceStore
    let onManageProject: (String, String) -> Void
    @State private var project: ProjectRecord?
    @State private var members: [ProjectMemberRecord] = []
    @State private var errorMessage: String?

    var body: some View {
        Form {
            Section("Organization project") {
                if let project {
                    LabeledContent("Name", value: project.name)
                    if !project.description.isEmpty {
                        LabeledContent("Description", value: project.description)
                    }
                    if store.canManageProjects {
                        Button("Manage Project…") { onManageProject(project.id, project.name) }
                    }
                } else if errorMessage == nil {
                    ProgressView().controlSize(.small)
                }
                if let errorMessage {
                    Text(errorMessage).foregroundStyle(.red).textSelection(.enabled)
                    Button("Try Again") { Task { await load() } }
                }
            }
            if !store.canManageProjects && !members.isEmpty {
                Section("Members") {
                    ForEach(members) { member in
                        UserIdentityLabel(account: member.user, displayName: member.user.displayName ?? member.user.email)
                    }
                }
            }
            ProjectLocalSetupSettings(store: store)
            ProjectMemoryCacheSettings(store: store)
        }
        .formStyle(.grouped)
        .frame(maxWidth: 760)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .task(id: store.activeProjectId) { await load() }
    }

    private func load() async {
        guard let id = store.activeProjectId else { return }
        project = nil
        members = []
        errorMessage = nil
        do {
            let result = try await store.projectRecord(id, refresh: true)
            guard !Task.isCancelled, store.activeProjectId == id else { return }
            project = result
            if !store.canManageProjects {
                let people = try await store.projectMemberDirectory(projectId: id)
                guard !Task.isCancelled, store.activeProjectId == id else { return }
                members = people
            }
        } catch is CancellationError {
            return
        } catch {
            guard store.activeProjectId == id else { return }
            errorMessage = error.localizedDescription
        }
    }
}

private struct ProjectLocalSetupSettings: View {
    @ObservedObject var store: WorkspaceStore
    @State private var bindings: [DaemonProjectBinding] = []
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var bindingToRemove: DaemonProjectBinding?

    var body: some View {
        Section("Repositories") {
            if isLoading, bindings.isEmpty {
                ProgressView()
                    .controlSize(.small)
            } else {
                ForEach(bindings) { binding in
                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(URL(fileURLWithPath: binding.workspaceRoot).lastPathComponent)
                                .lineLimit(1)
                            Text(binding.workspaceRoot)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .help(binding.workspaceRoot)
                        }
                        Spacer()
                        Menu {
                            Button("Reveal in Finder") {
                                NSWorkspace.shared.activateFileViewerSelecting([
                                    URL(fileURLWithPath: binding.workspaceRoot)
                                ])
                            }
                            Divider()
                            Button("Remove Repository", role: .destructive) {
                                bindingToRemove = binding
                            }
                        } label: {
                            Image(systemName: "ellipsis")
                        }
                        .menuIndicator(.hidden)
                        .menuStyle(.borderlessButton)
                        .fixedSize()
                        .help("Repository Actions")
                    }
                }
            }

            Button {
                chooseRepositories()
            } label: {
                Label("Add Repositories…", systemImage: "plus")
            }
            .disabled(store.activeProjectId == nil || isLoading)

            if let errorMessage {
                Text(errorMessage)
                    .textSelection(.enabled)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }

        .task(id: [store.activeProjectId ?? "", store.projectBindingsGeneration.uuidString]) {
            await load()
        }
        .confirmationDialog(
            "Remove Repository?",
            isPresented: Binding(
                get: { bindingToRemove != nil },
                set: { if !$0 { bindingToRemove = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Remove", role: .destructive) {
                guard let binding = bindingToRemove else { return }
                bindingToRemove = nil
                Task { await remove(binding) }
            }
            Button("Cancel", role: .cancel) {
                bindingToRemove = nil
            }
        } message: {
            Text("Clumsies will remove the Agent integrations managed in Settings and stop resolving this repository to the Project.")
        }
    }

    private func load() async {
        guard let projectId = store.activeProjectId else {
            bindings = []
            return
        }
        isLoading = true
        errorMessage = nil
        defer { isLoading = false }
        do {
            bindings = try await store.projectBindings(projectId)
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func chooseRepositories() {
        guard let projectId = store.activeProjectId else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.canCreateDirectories = true
        panel.prompt = "Add"
        panel.begin { response in
            guard response == .OK else { return }
            Task {
                isLoading = true
                errorMessage = nil
                do {
                    _ = try await store.addProjectRepositories(
                        panel.urls.map(\.path),
                        projectId: projectId
                    )
                    await load()
                } catch {
                    errorMessage = error.localizedDescription
                    isLoading = false
                }
            }
        }
    }

    private func remove(_ binding: DaemonProjectBinding) async {
        isLoading = true
        errorMessage = nil
        do {
            try await store.removeProjectRepository(binding)
            await load()
        } catch {
            errorMessage = error.localizedDescription
            isLoading = false
        }
    }
}
