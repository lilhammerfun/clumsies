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
                store.selectedSection = .memory
                store.showsProjectSettings = false
                await store.selectProject(id)
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
            if store.canCreateProject {
                Text("Create a Project to start organizing local memory.")
            } else {
                Text("Ask an organization administrator to grant you access to a Project.")
            }
        } actions: {
            if store.canCreateProject {
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
    let projectId: String
    var onDeleted: () -> Void = {}

    var body: some View {
        Form {
            if let project = store.administrationProject(id: projectId) {
                ProjectConfigurationSections(
                    store: store,
                    project: project,
                    allowsMutation: store.canMutateProject(projectId),
                    onDeleted: {
                        if store.activeProjectId == projectId { store.showsProjectSettings = false }
                        onDeleted()
                    }
                )
                .id(project.id)
                if projectId == store.activeProjectId {
                    ProjectLocalSetupSettings(store: store)
                    ProjectMemoryCacheSettings(store: store)
                }
            } else if let error = store.administrationProjectDetailStates[projectId]?.errorMessage {
                Text(error).foregroundStyle(.red).textSelection(.enabled)
                Button("Try Again") {
                    Task { await store.loadAdministrationProject(id: projectId, force: true) }
                }
            } else {
                ProgressView("Loading project…")
            }
        }
        .formStyle(.grouped)
        .frame(maxWidth: 760)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .task(id: "\(projectId):\(store.administrationRefreshGeneration)") {
            await store.loadAdministrationProject(id: projectId, force: true)
        }
    }
}

struct OrganizationProjectsView: View {
    @ObservedObject var store: WorkspaceStore
    @State private var path: [String] = []

    private var state: AdministrationPageState { store.administrationState(for: .projects) }

    var body: some View {
        NavigationStack(path: $path) {
            List {
                if state.isLoading { ProgressView("Loading projects…") }
                if store.administrationSnapshot?.projects.isEmpty == true, !state.isLoading {
                    Text("No projects yet.").foregroundStyle(.secondary)
                }
                ForEach(store.administrationSnapshot?.projects ?? []) { project in
                    NavigationLink(value: project.id) {
                        HStack {
                            Label(project.name, systemImage: "folder")
                            Spacer()
                            Text("\(project.memberCount) members").foregroundStyle(.secondary)
                        }
                    }
                }
                if state.nextCursor != nil {
                    Button("Show More") {
                        Task { await store.loadAdministration(section: .projects, loadMore: true) }
                    }
                    .disabled(state.isLoading)
                }
            }
            .navigationTitle("Organization Projects")
            .navigationDestination(for: String.self) { projectId in
                ProjectSettingsView(store: store, projectId: projectId, onDeleted: { path = [] })
                    .navigationTitle(store.administrationProject(id: projectId)?.name ?? "Project")
            }
        }
    }
}

private struct ProjectConfigurationSections: View {
    @ObservedObject var store: WorkspaceStore
    let project: AdminProjectRecord
    let allowsMutation: Bool
    let onDeleted: () -> Void
    @State private var showsEdit = false
    @State private var showsAddMember = false
    @State private var pendingMemberRemoval: ProjectMemberRecord?
    @State private var confirmsProjectDeletion = false
    @State private var errorMessage: String?

    var body: some View {
        Group {
            if let state = store.administrationProjectDetailStates[project.id], state.isStale {
                Section {
                    Text(state.errorMessage ?? "These project details are cached. Refresh before making changes.")
                        .foregroundStyle(.secondary)
                    Button("Try Again") {
                        Task { await store.loadAdministrationProject(id: project.id, force: true) }
                    }
                    .disabled(store.isMutatingAdministration || state.isLoading)
                }
            }
            Section {
                LabeledContent("Name") {
                    Text(project.name).textSelection(.enabled)
                    if store.canManageProject(project.id) {
                        Button("Edit…") { showsEdit = true }
                            .disabled(!allowsMutation)
                    }
                }
                if !project.description.isEmpty {
                    LabeledContent("Description") {
                        Text(project.description)
                            .fixedSize(horizontal: false, vertical: true)
                            .textSelection(.enabled)
                    }
                }
            }
            Section("Members") {
                if store.administrationProjectDetailStates[project.id]?.isLoading == true
                    || store.loadingAdministrationProjectIds.contains(project.id) {
                    ProgressView("Loading members…")
                        .controlSize(.small)
                } else if projectMembers.isEmpty {
                    Text("No project members.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(projectMembers) { member in
                        HStack(spacing: 10) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(member.user.displayName ?? member.user.email)
                                    .lineLimit(1)
                                if member.user.displayName != nil {
                                    Text(member.user.email)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            if store.canManageProject(project.id) {
                                Menu {
                                    Button("Remove Member…", role: .destructive) { pendingMemberRemoval = member }
                                        .disabled(!allowsMemberMutation)
                                } label: {
                                    Image(systemName: "ellipsis.circle")
                                }
                                .menuStyle(.borderlessButton)
                                .menuIndicator(.hidden)
                                .fixedSize()
                                .accessibilityLabel("Manage \(member.user.displayName ?? member.user.email)")
                            }
                        }
                    }
                }
                if store.canManageProject(project.id) {
                    Button("Add Member…") { showsAddMember = true }
                        .disabled(!allowsMemberMutation)
                }
            }
            Section {
                if store.canManageProject(project.id) {
                    Button("Delete Project…", role: .destructive) { confirmsProjectDeletion = true }
                        .disabled(!allowsMutation)
                }
                if let errorMessage { AdministrationInlineError(message: errorMessage) }
            }
        }
        .sheet(isPresented: $showsEdit) {
            ProjectDetailsSheet(
                store: store,
                project: project
            )
        }
        .sheet(isPresented: $showsAddMember) {
            ProjectMemberSheet(store: store, projectId: project.id)
        }
        .confirmationDialog("Delete project?", isPresented: $confirmsProjectDeletion) {
            Button("Delete \(project.name)", role: .destructive) {
                mutate { try await store.deleteAdminProject(project, onDeleted: onDeleted) }
            }
        } message: {
            Text("This permanently deletes the project and its project data.")
        }
        .confirmationDialog(
            "Remove project member?",
            isPresented: Binding(
                get: { pendingMemberRemoval != nil },
                set: { if !$0 { pendingMemberRemoval = nil } }
            ),
            presenting: pendingMemberRemoval
        ) { member in
            Button("Remove \(member.user.displayName ?? member.user.email)", role: .destructive) {
                mutate {
                    try await store.deleteAdminProjectMember(projectId: project.id, userId: member.id)
                }
                pendingMemberRemoval = nil
            }
        } message: { member in
            Text("\(member.user.email) will lose access to this project.")
        }
    }

    private var projectMembers: [ProjectMemberRecord] {
        store.administrationProjectMembers[project.id] ?? []
    }

    private var allowsMemberMutation: Bool {
        allowsMutation && !store.loadingAdministrationProjectIds.contains(project.id)
            && store.administrationProjectMembers[project.id] != nil
    }

    private func mutate(_ operation: @escaping () async throws -> Void) {
        guard allowsMutation else { return }
        errorMessage = nil
        Task {
            do { try await operation() }
            catch { errorMessage = error.localizedDescription }
        }
    }
}

private struct ProjectDetailsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: WorkspaceStore
    @State private var original: AdminProjectRecord
    @State private var name: String
    @State private var description: String
    @State private var errorMessage: String?

    init(
        store: WorkspaceStore,
        project: AdminProjectRecord
    ) {
        self.store = store
        _original = State(initialValue: project)
        _name = State(initialValue: project.name)
        _description = State(initialValue: project.description)
    }

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section("Project details") {
                    TextField("Name", text: $name)
                    TextField("Description", text: $description, axis: .vertical)
                        .lineLimit(3...6)
                }
                .disabled(store.isMutatingAdministration)
                if let errorMessage { AdministrationInlineError(message: errorMessage) }
            }
            .formStyle(.grouped)
            Divider()
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .disabled(store.isMutatingAdministration)
                Button("Save") { save() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canSave)
            }
            .padding(12)
        }
        .frame(width: 460, height: 280)
        .interactiveDismissDisabled(store.isMutatingAdministration)
    }

    private var hasChanges: Bool { name != original.name || description != original.description }
    private var canSave: Bool {
        let detail = store.administrationProjectDetailStates[original.id]
        return store.canMutateProject(original.id) && detail?.isStale != true && detail?.isLoading != true
            && hasChanges && ProjectMetadataValidation.isValid(name: name, description: description)
    }

    private func save() {
        guard canSave else { return }
        errorMessage = nil
        Task {
            do {
                _ = try await store.updateAdminProject(
                    original,
                    name: name.trimmingCharacters(in: .whitespacesAndNewlines),
                    description: description
                )
                dismiss()
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }
}

private struct ProjectMemberSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: WorkspaceStore
    let projectId: String
    @State private var query = ""
    @State private var members: [UserReference] = []
    @State private var selectedId: String?
    @State private var nextCursor: String?
    @State private var isLoading = true
    @State private var errorMessage: String?
    @State private var loadFailed = false
    @State private var loadMoreTask: Task<Void, Never>?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Add project member").font(.headline)
            ClassicSearchField(text: $query, prompt: "Search members", width: 404,
                accessibilityIdentifier: "project-member-search")
                .frame(height: 24)
                .disabled(store.isMutatingAdministration)
            List(selection: $selectedId) {
                ForEach(availableMembers) { member in
                    VStack(alignment: .leading, spacing: 3) {
                        Text(member.displayName ?? member.email)
                        if member.displayName != nil {
                            Text(member.email).foregroundStyle(.secondary)
                        }
                    }
                    .tag(member.id)
                }
            }
            .overlay {
                if availableMembers.isEmpty {
                    if isLoading {
                        ProgressView("Loading members…")
                    } else if errorMessage == nil {
                        Text("No available members.").foregroundStyle(.secondary)
                    }
                }
            }
            .disabled(store.isMutatingAdministration)
            if let errorMessage {
                HStack {
                    AdministrationInlineError(message: errorMessage)
                    if loadFailed {
                        Button("Try Again") {
                            loadMoreTask = Task { await loadMembers(cursor: members.isEmpty ? nil : nextCursor) }
                        }
                        .disabled(isLoading || store.isMutatingAdministration)
                    }
                }
            }
            HStack {
                if let nextCursor {
                    Button("Show More") {
                        loadMoreTask = Task { await loadMembers(cursor: nextCursor) }
                    }
                    .disabled(isLoading || store.isMutatingAdministration)
                }
                if isLoading && !members.isEmpty { ProgressView().controlSize(.small) }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .disabled(store.isMutatingAdministration)
                Button("Add") { add() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canAdd)
            }
        }
        .padding(18)
        .frame(width: 440, height: 430)
        .interactiveDismissDisabled(store.isMutatingAdministration)
        .onChange(of: query) { _, _ in
            loadMoreTask?.cancel()
            members = []
            selectedId = nil
            nextCursor = nil
            errorMessage = nil
            loadFailed = false
            isLoading = true
        }
        .task(id: query) {
            do {
                try await Task.sleep(for: .milliseconds(200))
                await loadMembers()
            } catch {}
        }
        .onDisappear { loadMoreTask?.cancel() }
    }

    private var availableMembers: [UserReference] {
        let existingIds = Set((store.administrationProjectMembers[projectId] ?? []).map(\.id))
        return members.filter { !existingIds.contains($0.id) }
    }

    private var canAdd: Bool {
        let detail = store.administrationProjectDetailStates[projectId]
        return store.canMutateProject(projectId) && detail?.isStale != true && detail?.isLoading != true
            && !store.loadingAdministrationProjectIds.contains(projectId)
            && store.administrationProjectMembers[projectId] != nil
            && availableMembers.contains { $0.id == selectedId }
    }

    private func loadMembers(cursor: String? = nil) async {
        let requestedQuery = query
        isLoading = true
        errorMessage = nil
        loadFailed = false
        defer { if requestedQuery == query && !Task.isCancelled { isLoading = false } }
        do {
            let response = try await store.searchProjectMemberCandidates(projectId: projectId, query: requestedQuery, cursor: cursor)
            try Task.checkCancellation()
            guard requestedQuery == query else { return }
            if cursor == nil { members = [] }
            let existingIds = Set(members.map(\.id))
            members.append(contentsOf: response.items.filter { !existingIds.contains($0.id) })
            nextCursor = response.pageInfo.nextCursor
        } catch is CancellationError {
        } catch {
            if requestedQuery == query && !Task.isCancelled {
                errorMessage = error.localizedDescription
                loadFailed = true
            }
        }
    }

    private func add() {
        guard canAdd, let selectedId else { return }
        errorMessage = nil
        loadFailed = false
        Task {
            do {
                try await store.addAdminProjectMember(projectId: projectId, userId: selectedId, role: .member)
                dismiss()
            } catch {
                errorMessage = error.localizedDescription
            }
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
        Section {
            Text("These repository bindings apply only on this Mac. Other members bind their own local folders.")
                .font(.caption)
                .foregroundStyle(.secondary)
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
        } header: {
            Text("Repositories on This Mac")
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
        bindings = []
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
