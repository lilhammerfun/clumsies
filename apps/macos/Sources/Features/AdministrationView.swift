import SwiftUI

enum AdministrationSection: String, CaseIterable, Identifiable, Sendable {
    case organization
    case members
    case projects
    case access
    case audit

    var id: String { rawValue }

    var title: String {
        switch self {
        case .organization: "Organization Details"
        case .members: "Members"
        case .projects: "Projects"
        case .access: "Sign-in & Access"
        case .audit: "Activity"
        }
    }

    var symbol: String {
        switch self {
        case .organization: "building.2"
        case .members: "person.2"
        case .projects: "folder"
        case .access: "key"
        case .audit: "list.bullet.clipboard"
        }
    }
}

struct AdministrationView: View {
    @ObservedObject var store: WorkspaceStore
    let section: AdministrationSection
    var projectId: String? = nil
    var onUnsavedChangesChange: (Bool) -> Void = { _ in }
    var onOpenProject: (AdminProjectRecord) -> Void = { _ in }
    var onDeletedProject: () -> Void = {}

    private var state: AdministrationPageState { store.administrationState(for: section) }

    var body: some View {
        VStack(spacing: 0) {
            if state.isLoaded, state.isStale {
                AdministrationStaleBanner()
            }
            if let errorMessage = state.errorMessage {
                AdministrationErrorBanner(message: errorMessage)
            }
            content
        }
        .font(.system(size: 13))
        .task(id: section) {
            if section != .members && section != .audit {
                await store.loadAdministration(section: section)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if section == .members {
            AdministrationMembersView(
                store: store,
                onUnsavedChangesChange: onUnsavedChangesChange
            )
        } else if section == .audit {
            AdministrationAuditView(store: store)
        } else if let snapshot = store.administrationSnapshot, state.isLoaded {
            switch section {
            case .organization:
                Form {
                    OrganizationNameSection(store: store, onUnsavedChangesChange: onUnsavedChangesChange)
                }
                .formStyle(.grouped)
            case .projects:
                AdministrationProjectsView(
                    store: store,
                    snapshot: snapshot,
                    allowsMutation: store.canMutateAdministration(.projects),
                    projectId: projectId,
                    onUnsavedChangesChange: onUnsavedChangesChange,
                    onOpenProject: onOpenProject,
                    onDeletedProject: onDeletedProject
                )
            case .access:
                AdministrationAccessView(
                    store: store,
                    snapshot: snapshot,
                    onUnsavedChangesChange: onUnsavedChangesChange
                )
            case .members, .audit:
                EmptyView()
            }
        } else if state.isLoading {
            ProgressView("Loading \(section.title)…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ContentUnavailableView(
                "\(section.title) Unavailable",
                systemImage: "building.2.crop.circle",
                description: Text(store.canAdministerOrganization
                    ? "Refresh to try again." : "Organization administrator access is required.")
            )
        }
    }
}

private struct AdministrationStaleBanner: View {
    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 2) {
                Text("Cached data")
                    .fontWeight(.semibold)
                Text("Changes on this page are disabled until a live refresh succeeds.")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(12)
        .background(Color.orange.opacity(0.12))
        .overlay(alignment: .bottom) { Divider() }
        .accessibilityElement(children: .combine)
    }
}

private struct AdministrationErrorBanner: View {
    let message: String

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.circle.fill")
                .foregroundStyle(.red)
            Text(message)
                .font(.system(size: 13))
                .textSelection(.enabled)
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Color.red.opacity(0.08))
        .overlay(alignment: .bottom) { Divider() }
    }
}

struct OrganizationNameSection: View {
    @ObservedObject var store: WorkspaceStore
    var onUnsavedChangesChange: (Bool) -> Void = { _ in }
    @State private var showsEdit = false

    var body: some View {
        Section {
            if let organization = store.administrationSnapshot?.organization {
                LabeledContent("Name") {
                    Text(organization.name)
                        .textSelection(.enabled)
                    Button("Edit…") { showsEdit = true }
                        .disabled(!store.canMutateAdministration(.organization))
                }
                .sheet(isPresented: $showsEdit) {
                    OrganizationEditSheet(
                        store: store,
                        organization: organization,
                        editsDomains: false,
                        onUnsavedChangesChange: onUnsavedChangesChange
                    )
                }
            } else if state.isLoading {
                ProgressView("Loading organization…")
            }
            if let errorMessage = state.errorMessage {
                AdministrationInlineError(message: errorMessage)
            }
            if state.isStale || state.errorMessage != nil || (!state.isLoaded && !state.isLoading) {
                HStack {
                    if state.errorMessage == nil {
                        Text(state.isStale ? "Refresh to edit organization details." : "Organization details are unavailable.")
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Refresh") {
                        Task { await store.loadAdministration(section: .organization, force: true) }
                    }
                    .disabled(state.isLoading || store.isMutatingAdministration || !store.canAdministerOrganization)
                }
            }
        }
    }

    private var state: AdministrationPageState { store.administrationState(for: .organization) }
}

private struct OrganizationEditSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: WorkspaceStore
    let editsDomains: Bool
    let onUnsavedChangesChange: (Bool) -> Void
    @State private var original: AdminOrganizationRecord
    @State private var value: String
    @State private var errorMessage: String?

    init(
        store: WorkspaceStore,
        organization: AdminOrganizationRecord,
        editsDomains: Bool,
        onUnsavedChangesChange: @escaping (Bool) -> Void
    ) {
        self.store = store
        self.editsDomains = editsDomains
        self.onUnsavedChangesChange = onUnsavedChangesChange
        _original = State(initialValue: organization)
        _value = State(initialValue: editsDomains
            ? organization.allowedEmailDomains.joined(separator: "\n") : organization.name)
    }

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    if editsDomains {
                        TextField("Domains", text: $value, axis: .vertical)
                            .lineLimit(3...6)
                    } else {
                        TextField("Name", text: $value)
                    }
                } header: {
                    Text(editsDomains ? "Allowed email domains" : "Organization name")
                } footer: {
                    if editsDomains {
                        Text("Enter one domain per line. Leave empty to allow any domain; people must still be added as members. Changes apply the next time a member signs in.")
                    }
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
        .frame(width: 460, height: editsDomains ? 320 : 210)
        .interactiveDismissDisabled(store.isMutatingAdministration)
        .onChange(of: hasChanges) { _, dirty in onUnsavedChangesChange(dirty) }
        .onDisappear { onUnsavedChangesChange(false) }
    }

    private var hasChanges: Bool {
        value != (editsDomains ? original.allowedEmailDomains.joined(separator: "\n") : original.name)
    }

    private var canSave: Bool {
        store.canMutateAdministration(.organization) && hasChanges
            && (editsDomains || !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }

    private func save() {
        guard canSave else { return }
        let domains = value.components(separatedBy: CharacterSet(charactersIn: ",\n"))
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
            .filter { !$0.isEmpty }
        errorMessage = nil
        Task {
            do {
                _ = try await store.updateAdminOrganization(
                    name: editsDomains ? original.name : value.trimmingCharacters(in: .whitespacesAndNewlines),
                    allowedEmailDomains: editsDomains ? Array(Set(domains)).sorted() : original.allowedEmailDomains,
                    expectedRevision: original.revision
                )
                dismiss()
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }
}

private struct AdministrationMembersView: View {
    @ObservedObject var store: WorkspaceStore
    let onUnsavedChangesChange: (Bool) -> Void
    @State private var query = ""
    @State private var completedQuery: String?
    @State private var showsAddMember = false
    @State private var pendingDisable: AdminOrganizationMemberRecord?
    @State private var errorMessage: String?

    var body: some View {
        Form {
            Section {
                ClassicSearchField(text: $query, prompt: "Search members", width: 300,
                    accessibilityIdentifier: "organization-members-search")
                    .frame(height: 24)
            }
            Section("Members") {
                if completedQuery != query || state.isLoading && members.isEmpty {
                    ProgressView("Loading members…")
                } else if members.isEmpty, state.errorMessage == nil {
                    Text(query.isEmpty ? "Add a member to get started." : "No members found.")
                        .foregroundStyle(.secondary)
                }
                ForEach(completedQuery == query ? members : []) { member in
                    let isCurrentUser = member.id == store.account?.userId
                    let ownerIsLocked = store.account?.role != AdminOrganizationRole.owner.rawValue
                        && member.role == .owner
                    let canEditMember = allowsMutation && !isCurrentUser && !ownerIsLocked

                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(member.displayName ?? member.email)
                                .lineLimit(1)
                            if member.displayName != nil {
                                Text(member.email)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                            }
                        }
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        VStack(alignment: .trailing, spacing: 3) {
                            Text(member.role.title + (isCurrentUser ? " · You" : ""))
                            if member.status == .disabled || !member.externalIdentityBound {
                                Text(member.status == .disabled ? "Disabled" : "Not signed in")
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .fixedSize()
                        Menu {
                            Picker("Role", selection: roleBinding(for: member)) {
                                ForEach(AdminOrganizationRole.allCases) { role in
                                    Text(role.title).tag(role)
                                        .disabled(!assignableRoles.contains(role))
                                }
                            }
                            .disabled(!canEditMember)
                            Divider()
                            if member.status == .disabled {
                                Button("Reactivate") {
                                    mutate { try await store.updateAdminOrganizationMember(member, status: .active) }
                                }
                                .disabled(!canEditMember)
                            } else {
                                Button("Disable Member…", role: .destructive) { pendingDisable = member }
                                    .disabled(!canEditMember)
                            }
                        } label: {
                            Image(systemName: "ellipsis.circle")
                        }
                        .menuStyle(.borderlessButton)
                        .menuIndicator(.hidden)
                        .fixedSize()
                        .accessibilityLabel("Manage \(member.displayName ?? member.email)")
                    }
                    .padding(.vertical, 2)
                }
                if completedQuery == query {
                    AdministrationLoadMore(store: store, section: .members, query: query)
                }
                Button("Add Member…") { showsAddMember = true }
                    .disabled(!allowsMutation)
            }
            if let errorMessage { AdministrationInlineError(message: errorMessage) }
        }
        .formStyle(.grouped)
        .onChange(of: query) { _, _ in
            completedQuery = nil
            errorMessage = nil
        }
        .task(id: query) {
            let requestedQuery = query
            do {
                try await Task.sleep(for: .milliseconds(200))
                await store.loadAdministration(section: .members, query: requestedQuery)
                try Task.checkCancellation()
                if requestedQuery == query { completedQuery = requestedQuery }
            } catch {}
        }
        .sheet(isPresented: $showsAddMember) {
            AdministrationAddMemberSheet(store: store, onUnsavedChangesChange: onUnsavedChangesChange)
        }
        .confirmationDialog(
            "Disable organization member?",
            isPresented: Binding(
                get: { pendingDisable != nil },
                set: { if !$0 { pendingDisable = nil } }
            ),
            presenting: pendingDisable
        ) { member in
            Button("Disable \(member.displayName ?? member.email)", role: .destructive) {
                mutate { try await store.disableAdminOrganizationMember(member) }
                pendingDisable = nil
            }
        } message: { member in
            Text("This disables \(member.email) and revokes their active sessions.")
        }
    }

    private var assignableRoles: [AdminOrganizationRole] {
        var roles: [AdminOrganizationRole] = [.member, .admin]
        if store.account?.role == AdminOrganizationRole.owner.rawValue {
            roles.append(.owner)
        }
        return roles
    }

    private var state: AdministrationPageState { store.administrationState(for: .members) }
    private var members: [AdminOrganizationMemberRecord] { store.administrationSnapshot?.members ?? [] }
    private var allowsMutation: Bool {
        completedQuery == query && store.canMutateAdministration(.members)
    }

    private func roleBinding(for member: AdminOrganizationMemberRecord) -> Binding<AdminOrganizationRole> {
        Binding(
            get: { member.role },
            set: { role in
                guard role != member.role else { return }
                mutate { try await store.updateAdminOrganizationMember(member, role: role) }
            }
        )
    }

    private func mutate(_ operation: @escaping () async throws -> Void) {
        guard allowsMutation else { return }
        errorMessage = nil
        Task {
            do {
                try await operation()
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }
}

private struct AdministrationAddMemberSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: WorkspaceStore
    let onUnsavedChangesChange: (Bool) -> Void
    @State private var email = ""
    @State private var role: AdminOrganizationRole = .member
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    TextField("Email", text: $email)
                        .textContentType(.emailAddress)
                    Picker("Organization role", selection: $role) {
                        ForEach(assignableRoles) { role in
                            Text(role.title).tag(role)
                        }
                    }
                } header: {
                    Text("Add member")
                } footer: {
                    Text("This person can sign in with this email using your organization's single sign-on. No invitation email is sent.")
                }
                .disabled(store.isMutatingAdministration)
                if let errorMessage {
                    AdministrationInlineError(message: errorMessage)
                }
            }
            .formStyle(.grouped)

            Divider()
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .disabled(store.isMutatingAdministration)
                Button("Add") { add() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canAdd)
            }
            .padding(12)
        }
        .frame(width: 460, height: 285)
        .interactiveDismissDisabled(store.isMutatingAdministration)
        .onChange(of: email.isEmpty) { _, empty in onUnsavedChangesChange(!empty) }
        .onDisappear { onUnsavedChangesChange(false) }
    }

    private var canAdd: Bool {
        store.canMutateAdministration(.members)
            && email.contains("@")
            && !email.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var assignableRoles: [AdminOrganizationRole] {
        var roles: [AdminOrganizationRole] = [.member, .admin]
        if store.account?.role == AdminOrganizationRole.owner.rawValue {
            roles.append(.owner)
        }
        return roles
    }

    private func add() {
        guard canAdd else { return }
        errorMessage = nil
        Task {
            do {
                try await store.inviteAdminOrganizationMember(
                    email: email.trimmingCharacters(in: .whitespacesAndNewlines),
                    role: role
                )
                dismiss()
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }
}

private struct AdministrationProjectsView: View {
    @ObservedObject var store: WorkspaceStore
    let snapshot: AdministrationSnapshot
    let allowsMutation: Bool
    let projectId: String?
    let onUnsavedChangesChange: (Bool) -> Void
    let onOpenProject: (AdminProjectRecord) -> Void
    let onDeletedProject: () -> Void
    @State private var showsCreation = false

    var body: some View {
        Group {
            if let projectId {
                if let project = store.administrationProject(id: projectId) {
                    AdministrationProjectEditor(
                        store: store,
                        project: project,
                        allowsMutation: allowsMutation
                            && store.administrationProjectDetailStates[projectId]?.isStale != true
                            && store.administrationProjectDetailStates[projectId]?.isLoading != true,
                        onUnsavedChangesChange: onUnsavedChangesChange,
                        onDeleted: onDeletedProject
                    )
                    .id(project.id)
                } else {
                    Form {
                        if let error = store.administrationProjectDetailStates[projectId]?.errorMessage {
                            Text(error).foregroundStyle(.secondary)
                            Button("Try Again") {
                                Task { await store.loadAdministrationProject(id: projectId, force: true) }
                            }
                        } else {
                            ProgressView("Loading project…")
                        }
                    }
                    .formStyle(.grouped)
                }
            } else {
                Form {
                    Section("Projects") {
                        if snapshot.projects.isEmpty {
                            Text("Create a project to share memories with your team.")
                                .foregroundStyle(.secondary)
                        }
                        ForEach(snapshot.projects) { project in
                            Button { onOpenProject(project) } label: {
                                HStack(spacing: 10) {
                                    Image(systemName: "folder")
                                        .foregroundStyle(.secondary)
                                    Text(project.name)
                                        .lineLimit(1)
                                    Spacer()
                                    Text("\(project.memberCount) members")
                                        .foregroundStyle(.secondary)
                                    Image(systemName: "chevron.right")
                                        .foregroundStyle(.tertiary)
                                }
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                        AdministrationLoadMore(store: store, section: .projects)
                        Button("Create Project…") { showsCreation = true }
                            .disabled(!allowsMutation)
                    }
                }
                .formStyle(.grouped)
            }
        }
        .task(id: "\(projectId ?? ""):\(store.administrationRefreshGeneration.uuidString)") {
            if let projectId { await store.loadAdministrationProject(id: projectId) }
        }
        .sheet(isPresented: $showsCreation) {
            ProjectCreationSheet(
                store: store,
                onCreated: { id in
                    await store.loadAdministration(section: .projects, force: true)
                    if store.administrationProject(id: id) == nil {
                        await store.loadAdministrationProject(id: id)
                    }
                    onUnsavedChangesChange(false)
                    if let project = store.administrationProject(id: id) { onOpenProject(project) }
                },
                onUnsavedChangesChange: onUnsavedChangesChange
            )
        }
    }
}

private struct AdministrationProjectEditor: View {
    @ObservedObject var store: WorkspaceStore
    let project: AdminProjectRecord
    let allowsMutation: Bool
    let onUnsavedChangesChange: (Bool) -> Void
    let onDeleted: () -> Void
    @State private var showsEdit = false
    @State private var showsAddMember = false
    @State private var pendingMemberRemoval: ProjectMemberRecord?
    @State private var confirmsProjectDeletion = false
    @State private var errorMessage: String?

    var body: some View {
        Form {
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
                    Button("Edit…") { showsEdit = true }
                        .disabled(!allowsMutation)
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
                if store.loadingAdministrationProjectIds.contains(project.id) {
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
                Button("Add Member…") { showsAddMember = true }
                    .disabled(!allowsMemberMutation)
            }
            Section {
                Button("Delete Project…", role: .destructive) { confirmsProjectDeletion = true }
                    .disabled(!allowsMutation)
                if let errorMessage { AdministrationInlineError(message: errorMessage) }
            }
        }
        .formStyle(.grouped)
        .task(id: "\(project.id):\(store.administrationRefreshGeneration.uuidString)") {
            await store.loadAdministrationProjectMembers(projectId: project.id)
        }
        .sheet(isPresented: $showsEdit) {
            AdministrationProjectEditSheet(
                store: store,
                project: project,
                onUnsavedChangesChange: onUnsavedChangesChange
            )
        }
        .sheet(isPresented: $showsAddMember) {
            AdministrationProjectMemberSheet(store: store, projectId: project.id)
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

private struct AdministrationProjectEditSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: WorkspaceStore
    let onUnsavedChangesChange: (Bool) -> Void
    @State private var original: AdminProjectRecord
    @State private var name: String
    @State private var description: String
    @State private var errorMessage: String?

    init(
        store: WorkspaceStore,
        project: AdminProjectRecord,
        onUnsavedChangesChange: @escaping (Bool) -> Void
    ) {
        self.store = store
        self.onUnsavedChangesChange = onUnsavedChangesChange
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
        .onChange(of: hasChanges) { _, dirty in onUnsavedChangesChange(dirty) }
        .onDisappear { onUnsavedChangesChange(false) }
    }

    private var hasChanges: Bool { name != original.name || description != original.description }
    private var canSave: Bool {
        let detail = store.administrationProjectDetailStates[original.id]
        return store.canMutateAdministration(.projects) && detail?.isStale != true && detail?.isLoading != true
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

private struct AdministrationProjectMemberSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: WorkspaceStore
    let projectId: String
    @State private var query = ""
    @State private var members: [AdminOrganizationMemberRecord] = []
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

    private var availableMembers: [AdminOrganizationMemberRecord] {
        let existingIds = Set((store.administrationProjectMembers[projectId] ?? []).map(\.id))
        return members.filter { $0.status != .disabled && !existingIds.contains($0.id) }
    }

    private var canAdd: Bool {
        let detail = store.administrationProjectDetailStates[projectId]
        return store.canMutateAdministration(.projects) && detail?.isStale != true && detail?.isLoading != true
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
            let response = try await store.searchAdministrationMembers(query: requestedQuery, cursor: cursor)
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

private struct AdministrationAccessView: View {
    @ObservedObject var store: WorkspaceStore
    let snapshot: AdministrationSnapshot
    let onUnsavedChangesChange: (Bool) -> Void
    @State private var showsDomainEdit = false

    var body: some View {
        Form {
            Section("Sign-in") {
                if let provider = snapshot.identityProvider {
                    LabeledContent("Single sign-on", value: provider.configured ? "Configured" : "Not configured")
                }
            }
            if let organization = snapshot.organization {
                Section {
                    LabeledContent("Allowed email domains") {
                        Text(organization.allowedEmailDomains.isEmpty
                            ? "Any domain" : organization.allowedEmailDomains.joined(separator: ", "))
                            .fixedSize(horizontal: false, vertical: true)
                        Button("Edit…") { showsDomainEdit = true }
                            .disabled(!store.canMutateAdministration(.organization))
                    }
                } footer: {
                    Text("People must be added as organization members before they can sign in.")
                }
                .sheet(isPresented: $showsDomainEdit) {
                    OrganizationEditSheet(
                        store: store,
                        organization: organization,
                        editsDomains: true,
                        onUnsavedChangesChange: onUnsavedChangesChange
                    )
                }
            }
        }
        .formStyle(.grouped)
    }
}

private struct AdministrationAuditView: View {
    @ObservedObject var store: WorkspaceStore
    @State private var query = ""
    @State private var completedQuery: String?

    var body: some View {
        Form {
            Section {
                ClassicSearchField(text: $query, prompt: "Search activity", width: 300,
                    accessibilityIdentifier: "organization-audit-events-search")
                    .frame(height: 24)
            }
            Section("Activity") {
                if completedQuery != query || state.isLoading && events.isEmpty {
                    ProgressView("Loading activity…")
                } else if events.isEmpty, state.errorMessage == nil {
                    Text(query.isEmpty ? "No activity yet." : "No activity found.")
                        .foregroundStyle(.secondary)
                }
                ForEach(completedQuery == query ? events : []) { event in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(actionTitle(event.action))
                        Text(targetName(event))
                            .foregroundStyle(.secondary)
                        HStack(alignment: .firstTextBaseline) {
                            Text(actorName(event))
                            Spacer(minLength: 8)
                            Text(TimestampFormatting.absoluteText(event.createdAt) ?? event.createdAt)
                                .multilineTextAlignment(.trailing)
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                    .padding(.vertical, 3)
                    .textSelection(.enabled)
                    .accessibilityElement(children: .combine)
                }
                if completedQuery == query {
                    AdministrationLoadMore(store: store, section: .audit, query: query)
                }
            }
        }
        .formStyle(.grouped)
        .onChange(of: query) { _, _ in completedQuery = nil }
        .task(id: query) {
            let requestedQuery = query
            do {
                try await Task.sleep(for: .milliseconds(200))
                await store.loadAdministration(section: .audit, query: requestedQuery)
                try Task.checkCancellation()
                if requestedQuery == query { completedQuery = requestedQuery }
            } catch {}
        }
    }

    private var state: AdministrationPageState { store.administrationState(for: .audit) }
    private var events: [AdminAuditEventRecord] { store.administrationSnapshot?.auditEvents ?? [] }

    private func actionTitle(_ action: String) -> String {
        switch action {
        case "admin.org_updated": "Updated organization"
        case "admin.member_created": "Added member"
        case "admin.member_updated": "Updated member"
        case "admin.project_created": "Created project"
        case "admin.project_updated": "Updated project"
        case "admin.project_deleted": "Deleted project"
        case "admin.project_member_created": "Added project member"
        case "admin.project_member_updated": "Updated project member"
        case "admin.project_member_deleted": "Removed project member"
        case "admin.token_revoked": "Revoked sign-in credential"
        default: "Organization activity"
        }
    }

    private func targetName(_ event: AdminAuditEventRecord) -> String {
        if let name = event.targetDisplayName, !name.isEmpty { return name }
        switch event.targetType {
        case "org": return store.organization?.name ?? "Unavailable organization"
        case "user": return "Unavailable member"
        case "project": return "Unavailable project"
        case "project_member": return "Unavailable project member"
        case "access_token": return "Unavailable sign-in credential"
        default: return "Unavailable item"
        }
    }

    private func actorName(_ event: AdminAuditEventRecord) -> String {
        if let name = event.actorDisplayName ?? event.actorEmail { return name }
        return event.actorUserId == nil ? "System" : "Unavailable member"
    }
}

private struct AdministrationLoadMore: View {
    @ObservedObject var store: WorkspaceStore
    let section: AdministrationSection
    var query: String? = nil

    var body: some View {
        let state = store.administrationState(for: section)
        if state.nextCursor != nil {
            HStack {
                Button("Show More") {
                    Task { await store.loadAdministration(section: section, loadMore: true, query: query) }
                }
                .disabled(state.isLoading)
                if state.isLoading { ProgressView().controlSize(.small) }
                Spacer()
            }
            .padding(.vertical, 4)
        }
    }
}

private struct AdministrationInlineError: View {
    let message: String

    var body: some View {
        Label(message, systemImage: "exclamationmark.circle")
            .font(.system(size: 13))
            .foregroundStyle(.red)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
    }
}
