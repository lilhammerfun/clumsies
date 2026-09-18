import AppKit
import SwiftUI

struct ProjectCreationSheet: View {
    @Environment(\.workspaceActions) private var workspaceActions
    @EnvironmentObject private var bundleStore: BundleStore
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation
    @EnvironmentObject private var projectService: ProjectService
    @Environment(\.dismiss) private var dismiss
    @FocusState private var nameFocused: Bool
    @StateObject private var model: ProjectCreationModel
    @State private var showsOptions = false

    init(model: @autoclosure @escaping () -> ProjectCreationModel) {
        _model = StateObject(wrappedValue: model())
    }

    var body: some View {
        VStack(spacing: 0) {
            Text("New Project")
                .font(.headline)
                .padding(.top, 20)
            Form {
                Section {
                    TextField("Name", text: self.$model.name).focused(self.$nameFocused)
                    TextField("Description", text: self.$model.description, axis: .vertical)
                        .lineLimit(2...4)
                }
                Section {
                    DisclosureGroup("Additional options", isExpanded: self.$showsOptions) {
                        Picker("Initial memory", selection: self.$model.selectedBundleId) {
                            Text("None").tag(Optional<String>.none)
                            ForEach(self.bundleStore.bundles) { bundle in
                                Text(bundle.name).tag(Optional(bundle.id))
                            }
                        }
                        ForEach(self.model.repositories, id: \.path) { repository in
                            HStack {
                                Text(repository.lastPathComponent).lineLimit(1)
                                    .help(repository.path)
                                Spacer()
                                Button {
                                    self.model.repositories.removeAll { $0 == repository }
                                } label: { Image(systemName: "minus.circle") }
                                    .buttonStyle(.borderless)
                                    .accessibilityLabel("Remove \(repository.lastPathComponent)")
                            }
                        }
                        Button("Attach Repositories…") { self.chooseRepositories() }
                        Text("Repositories can also be attached later on this Mac.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
                if let errorMessage = model.errorMessage {
                    Text(errorMessage).foregroundStyle(.red).textSelection(.enabled)
                }
            }
            .formStyle(.grouped)
            .disabled(self.model.isCreating)
            HStack {
                if self.model.isCreating { ProgressView().controlSize(.small) }
                Spacer()
                Button("Cancel", role: .cancel) { self.dismiss() }
                    .keyboardShortcut(.cancelAction).disabled(self.model.isCreating)
                Button("Create") { self.create() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!ProjectMetadataValidation.isValid(name: self.model.name, description: self.model.description) || self.model.isCreating)
            }
            .padding(16)
        }
        .frame(width: 480, height: showsOptions ? 500 : 320)
        .interactiveDismissDisabled(model.isCreating)
        .onAppear { self.nameFocused = true }
    }

    private func chooseRepositories() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.prompt = "Attach"
        panel.begin { response in
            guard response == .OK else { return }
            let existing = Set(model.repositories.map(\.standardized.path))
            self.model.repositories += panel.urls.map(\.standardized).filter { !existing.contains($0.path) }
            self.model.repositories.sort { $0.path < $1.path }
        }
    }

    private func create() {
        Task {
            guard let id = await model.create() else { return }
            workspaceNavigation.selectedSection = .memory
            workspaceNavigation.showsProjectSettings = false
            await workspaceActions.selectProject(id)
            dismiss()
        }
    }
}

struct ProjectUnavailableView: View {
    @Environment(\.workspaceActions) private var workspaceActions
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation

    var body: some View {
        ContentUnavailableView {
            Label("No Projects", systemImage: "folder")
        } description: {
            if self.workspaceContext.canCreateProject {
                Text("Create a Project to start organizing local memory.")
            } else {
                Text("Ask an organization administrator to grant you access to a Project.")
            }
        } actions: {
            if self.workspaceContext.canCreateProject {
                Button("New Project…") {
                    self.workspaceNavigation.presentProjectCreation()
                }
                .keyboardShortcut(.defaultAction)
            } else {
                Button("Refresh") {
                    Task { await workspaceActions.reload() }
                }
            }
        }
    }
}

struct ProjectSettingsView: View {
    @EnvironmentObject private var projectService: ProjectService
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation
    @EnvironmentObject private var administration: AdministrationModel
    let projectId: String
    var onDeleted: () -> Void = {}

    var body: some View {
        Form {
            if let project = administration.project(id: projectId) {
                ProjectConfigurationSections(project: project,
                    allowsMutation: self.administration.canMutateProject(self.projectId),
                    onDeleted: {
                        if self.workspaceContext.activeProjectId == self.projectId { self.workspaceNavigation.showsProjectSettings = false }
                        self.onDeleted()
                    }
                )
                .id(project.id)
                if self.projectId == self.workspaceContext.activeProjectId {
                    ProjectLocalSetupSettings(model: ProjectRepositoriesModel(context: workspaceContext, projects: projectService))
                    ProjectMemoryCacheSettings(model: ProjectStorageModel(context: workspaceContext))
                }
            } else if let error = administration.projectDetailStates[projectId]?.errorMessage {
                Text(error).foregroundStyle(.red).textSelection(.enabled)
                Button("Try Again") {
                    Task { await self.administration.loadProject(id: self.projectId, force: true) }
                }
            } else {
                ProgressView("Loading project…")
            }
        }
        .formStyle(.grouped)
        .frame(maxWidth: 760)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .task(id: "\(projectId):\(administration.refreshGeneration)") {
            await self.administration.loadProject(id: self.projectId, force: true)
        }
    }
}

struct OrganizationProjectsView: View {
    @EnvironmentObject private var administration: AdministrationModel
    @State private var path: [String] = []

    private var state: AdministrationPageState { administration.state(for: .projects) }

    var body: some View {
        NavigationStack(path: $path) {
            List {
                if self.state.isLoading { ProgressView("Loading projects…") }
                if self.administration.snapshot?.projects.isEmpty == true, !self.state.isLoading {
                    Text("No projects yet.").foregroundStyle(.secondary)
                }
                ForEach(self.administration.snapshot?.projects ?? []) { project in
                    NavigationLink(value: project.id) {
                        HStack {
                            Label(project.name, systemImage: "folder")
                            Spacer()
                            Text("\(project.memberCount) members").foregroundStyle(.secondary)
                        }
                    }
                }
                if self.state.nextCursor != nil {
                    Button("Show More") {
                        Task { await self.administration.load(section: .projects, loadMore: true) }
                    }
                    .disabled(self.state.isLoading)
                }
            }
            .navigationTitle("Organization Projects")
            .navigationDestination(for: String.self) { projectId in
                ProjectSettingsView(projectId: projectId, onDeleted: { self.path = [] })
                    .navigationTitle(self.administration.project(id: projectId)?.name ?? "Project")
            }
        }
    }
}

private struct ProjectConfigurationSections: View {
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
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
            if let state = administration.projectDetailStates[project.id], state.isStale {
                Section {
                    Text(state.errorMessage ?? "These project details are cached. Refresh before making changes.")
                        .foregroundStyle(.secondary)
                    Button("Try Again") {
                        Task { await self.administration.loadProject(id: self.project.id, force: true) }
                    }
                    .disabled(self.workspaceContext.isMutatingAdministration || state.isLoading)
                }
            }
            Section {
                LabeledContent("Name") {
                    Text(self.project.name).textSelection(.enabled)
                    if self.workspaceContext.canManageProject(self.project.id) {
                        Button("Edit…") { self.showsEdit = true }
                            .disabled(!self.allowsMutation)
                    }
                }
                if !self.project.description.isEmpty {
                    LabeledContent("Description") {
                        Text(self.project.description)
                            .fixedSize(horizontal: false, vertical: true)
                            .textSelection(.enabled)
                    }
                }
            }
            Section("Members") {
                if self.administration.projectDetailStates[self.project.id]?.isLoading == true
                    || self.administration.loadingProjectIds.contains(self.project.id) {
                    ProgressView("Loading members…")
                        .controlSize(.small)
                } else if self.projectMembers.isEmpty {
                    Text("No project members.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(self.projectMembers) { member in
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
                            if self.workspaceContext.canManageProject(self.project.id) {
                                Menu {
                                    Button("Remove Member…", role: .destructive) { self.pendingMemberRemoval = member }
                                        .disabled(!self.allowsMemberMutation)
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
                if self.workspaceContext.canManageProject(self.project.id) {
                    Button("Add Member…") { self.showsAddMember = true }
                        .disabled(!self.allowsMemberMutation)
                }
            }
            Section {
                if self.workspaceContext.canManageProject(self.project.id) {
                    Button("Delete Project…", role: .destructive) { self.confirmsProjectDeletion = true }
                        .disabled(!self.allowsMutation)
                }
                if let errorMessage { AdministrationInlineError(message: errorMessage) }
            }
        }
        .sheet(isPresented: $showsEdit) {
            ProjectDetailsSheet(project: self.project
            )
        }
        .sheet(isPresented: $showsAddMember) {
            ProjectMemberSheet(projectId: self.project.id, administration: self.administration)
        }
        .confirmationDialog("Delete project?", isPresented: $confirmsProjectDeletion) {
            Button("Delete \(self.project.name)", role: .destructive) {
                self.mutate { try await self.administration.deleteAdminProject(self.project, onDeleted: self.onDeleted) }
            }
        } message: {
            Text("This permanently deletes the project and its project data.")
        }
        .confirmationDialog(
            "Remove project member?",
            isPresented: Binding(
                get: { self.pendingMemberRemoval != nil },
                set: { if !$0 { self.pendingMemberRemoval = nil } }
            ),
            presenting: pendingMemberRemoval
        ) { member in
            Button("Remove \(member.user.displayName ?? member.user.email)", role: .destructive) {
                self.mutate {
                    try await self.administration.deleteAdminProjectMember(projectId: self.project.id, userId: member.id)
                }
                self.pendingMemberRemoval = nil
            }
        } message: { member in
            Text("\(member.user.email) will lose access to this project.")
        }
    }

    private var projectMembers: [ProjectMemberRecord] {
        administration.projectMembers[project.id] ?? []
    }

    private var allowsMemberMutation: Bool {
        allowsMutation && !administration.loadingProjectIds.contains(project.id)
            && administration.projectMembers[project.id] != nil
    }

    private func mutate(_ operation: @escaping () async throws -> Void) {
        guard allowsMutation else { return }
        errorMessage = nil
        Task {
            do { try await operation() }
            catch { self.errorMessage = error.localizedDescription }
        }
    }
}

private struct ProjectDetailsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
    @State private var original: AdminProjectRecord
    @State private var name: String
    @State private var description: String
    @State private var errorMessage: String?

    init(
        project: AdminProjectRecord
    ) {
        _original = State(initialValue: project)
        _name = State(initialValue: project.name)
        _description = State(initialValue: project.description)
    }

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section("Project details") {
                    TextField("Name", text: self.$name)
                    TextField("Description", text: self.$description, axis: .vertical)
                        .lineLimit(3...6)
                }
                .disabled(self.workspaceContext.isMutatingAdministration)
                if let errorMessage { AdministrationInlineError(message: errorMessage) }
            }
            .formStyle(.grouped)
            Divider()
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { self.dismiss() }
                    .disabled(self.workspaceContext.isMutatingAdministration)
                Button("Save") { self.save() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!self.canSave)
            }
            .padding(12)
        }
        .frame(width: 460, height: 280)
        .interactiveDismissDisabled(workspaceContext.isMutatingAdministration)
    }

    private var hasChanges: Bool { name != original.name || description != original.description }
    private var canSave: Bool {
        let detail = administration.projectDetailStates[original.id]
        return administration.canMutateProject(original.id) && detail?.isStale != true && detail?.isLoading != true
            && hasChanges && ProjectMetadataValidation.isValid(name: name, description: description)
    }

    private func save() {
        guard canSave else { return }
        errorMessage = nil
        Task {
            do {
                _ = try await self.administration.updateAdminProject(
                    self.original,
                    name: self.name.trimmingCharacters(in: .whitespacesAndNewlines),
                    description: self.description
                )
                self.dismiss()
            } catch {
                self.errorMessage = error.localizedDescription
            }
        }
    }
}

private struct ProjectMemberSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
    let projectId: String
    @StateObject private var model: ProjectMemberPickerModel

    init(projectId: String, administration: AdministrationModel) {
        self.projectId = projectId
        _model = StateObject(wrappedValue: ProjectMemberPickerModel(projectId: projectId, administration: administration))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Add project member").font(.headline)
            ClassicSearchField(text: self.$model.query, prompt: "Search members", width: 404,
                accessibilityIdentifier: "project-member-search")
                .frame(height: 24)
                .disabled(self.workspaceContext.isMutatingAdministration)
            List(selection: self.$model.selectedId) {
                ForEach(self.model.availableMembers) { member in
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
                if self.model.availableMembers.isEmpty {
                    if self.model.isLoading {
                        ProgressView("Loading members…")
                    } else if model.errorMessage == nil {
                        Text("No available members.").foregroundStyle(.secondary)
                    }
                }
            }
            .disabled(self.workspaceContext.isMutatingAdministration)
            if let errorMessage = model.errorMessage {
                HStack {
                    AdministrationInlineError(message: errorMessage)
                    if self.model.loadFailed {
                        Button("Try Again") {
                            self.model.retry()
                        }
                        .disabled(self.model.isLoading || self.workspaceContext.isMutatingAdministration)
                    }
                }
            }
            HStack {
                if model.nextCursor != nil {
                    Button("Show More") {
                        self.model.loadMore()
                    }
                    .disabled(self.model.isLoading || self.workspaceContext.isMutatingAdministration)
                }
                if self.model.isLoading && !self.model.members.isEmpty { ProgressView().controlSize(.small) }
                Spacer()
                Button("Cancel", role: .cancel) { self.dismiss() }
                    .disabled(self.workspaceContext.isMutatingAdministration)
                Button("Add") { Task { if await self.model.add() { self.dismiss() } } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!self.model.canAdd)
            }
        }
        .padding(18)
        .frame(width: 440, height: 430)
        .interactiveDismissDisabled(workspaceContext.isMutatingAdministration)
        .task(id: model.searchGeneration) {
            await self.model.search()
        }
        .onDisappear { self.model.cancel() }
    }

}

private struct ProjectLocalSetupSettings: View {
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var projectService: ProjectService
    @StateObject private var model: ProjectRepositoriesModel
    @State private var bindingToRemove: DaemonProjectBinding?

    init(model: @autoclosure @escaping () -> ProjectRepositoriesModel) {
        _model = StateObject(wrappedValue: model())
    }

    var body: some View {
        Section {
            Text("These repository bindings apply only on this Mac. Other members bind their own local folders.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if self.model.isLoading, self.model.bindings.isEmpty {
                ProgressView()
                    .controlSize(.small)
            } else {
                ForEach(self.model.bindings) { binding in
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
                                self.bindingToRemove = binding
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
                self.chooseRepositories()
            } label: {
                Label("Add Repositories…", systemImage: "plus")
            }
            .disabled(self.workspaceContext.activeProjectId == nil || self.model.isLoading)

            if let errorMessage = model.errorMessage {
                Text(errorMessage)
                    .textSelection(.enabled)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        } header: {
            Text("Repositories on This Mac")
        }
        .task(id: [workspaceContext.activeProjectId ?? "", projectService.projectBindingsGeneration.uuidString]) {
            await self.model.load()
        }
        .confirmationDialog(
            "Remove Repository?",
            isPresented: Binding(
                get: { self.bindingToRemove != nil },
                set: { if !$0 { self.bindingToRemove = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Remove", role: .destructive) {
                guard let binding = bindingToRemove else { return }
                self.bindingToRemove = nil
                Task { await self.model.remove(binding) }
            }
            Button("Cancel", role: .cancel) {
                self.bindingToRemove = nil
            }
        } message: {
            Text("Clumsies will remove the Agent integrations managed in Settings and stop resolving this repository to the Project.")
        }
    }

    private func chooseRepositories() {
        guard let projectId = workspaceContext.activeProjectId else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.canCreateDirectories = true
        panel.prompt = "Add"
        panel.begin { response in
            guard response == .OK else { return }
            Task { await model.add(panel.urls, projectId: projectId) }
        }
    }

}
