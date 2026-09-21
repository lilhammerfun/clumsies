import SwiftUI

struct AdministrationView: View {
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
    let section: AdministrationSection
    var onUnsavedChangesChange: (Bool) -> Void = { _ in }

    private var state: AdministrationPageState { administration.state(for: section) }

    var body: some View {
        content
        .pageFeedback(state.isLoaded && state.isStale
            ? String(localized: "Changes on this page are disabled until a live refresh succeeds.") : nil, isStatus: true) {
            Task { await administration.load(section: section, force: true) }
        }
        .pageFeedback(state.isLoaded ? state.errorMessage : nil, isStatus: true) {
            Task { await administration.load(section: section, force: true) }
        }
        .font(.system(size: 13))
        .task(id: section) {
            if section != .members && section != .audit {
                await administration.load(section: section)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if section == .members {
            AdministrationMembersView(onUnsavedChangesChange: onUnsavedChangesChange
            )
        } else if section == .audit {
            AdministrationAuditView()
        } else if let snapshot = administration.snapshot, state.isLoaded {
            switch section {
            case .organization:
                Form {
                    OrganizationNameSection(onUnsavedChangesChange: onUnsavedChangesChange)
                }
                .formStyle(.grouped)
            case .projects:
                OrganizationProjectsView()
            case .access:
                AdministrationAccessView(snapshot: snapshot,
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
                description: Text(state.errorMessage ?? (workspaceContext.canAdministerOrganization
                    ? String(localized: "Refresh to try again.") : String(localized: "Organization administrator access is required.")))
            )
        }
    }
}



struct OrganizationNameSection: View {
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
    var onUnsavedChangesChange: (Bool) -> Void = { _ in }
    @State private var showsEdit = false

    var body: some View {
        Section {
            if let organization = administration.snapshot?.organization {
                LabeledContent("Name") {
                    Text(organization.name)
                        .textSelection(.enabled)
                    Button("Edit…") { showsEdit = true }
                        .disabled(!administration.canMutate(.organization))
                }
                .sheet(isPresented: $showsEdit) {
                    OrganizationEditSheet(organization: organization,
                        editsDomains: false,
                        onUnsavedChangesChange: onUnsavedChangesChange
                    )
                }
            } else if state.isLoading {
                ProgressView("Loading organization…")
            }
        }
        .pageFeedback(state.errorMessage) { Task { await administration.load(section: .organization, force: true) } }
    }

    private var state: AdministrationPageState { administration.state(for: .organization) }
}

private struct OrganizationEditSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
    let editsDomains: Bool
    let onUnsavedChangesChange: (Bool) -> Void
    @State private var original: AdminOrganizationRecord
    @State private var value: String
    @State private var errorMessage: String?

    init(
        organization: AdminOrganizationRecord,
        editsDomains: Bool,
        onUnsavedChangesChange: @escaping (Bool) -> Void
    ) {
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
                    VStack(alignment: .leading, spacing: 8) {
                        if editsDomains {
                            Text("Enter one domain per line. Leave empty to allow any domain; people must still be added as members. Changes apply the next time a member signs in.")
                        }
                        FormErrorMessage(message: errorMessage)
                    }
                }
                .disabled(workspaceContext.isMutatingAdministration)
            }
            .formStyle(.grouped)
            SheetActionBar(
                confirmationTitle: Text("Save"), progressTitle: "Saving…",
                isWorking: workspaceContext.isMutatingAdministration, canConfirm: canSave,
                cancel: { dismiss() }, confirm: save
            )
        }
        .frame(width: 460, height: editsDomains ? 320 : 210)
        .interactiveDismissDisabled(workspaceContext.isMutatingAdministration)
        .onChange(of: hasChanges) { _, dirty in onUnsavedChangesChange(dirty) }
        .onDisappear { onUnsavedChangesChange(false) }
    }

    private var hasChanges: Bool {
        value != (editsDomains ? original.allowedEmailDomains.joined(separator: "\n") : original.name)
    }

    private var canSave: Bool {
        administration.canMutate(.organization) && hasChanges
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
                _ = try await administration.updateAdminOrganization(
                    name: editsDomains ? original.name : value.trimmingCharacters(in: .whitespacesAndNewlines),
                    allowedEmailDomains: editsDomains ? Array(Set(domains)).sorted() : original.allowedEmailDomains,
                    expectedRevision: original.revision
                )
                dismiss()
            } catch {
                errorMessage = error.actionMessage
            }
        }
    }
}

private struct AdministrationMembersView: View {
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
    let onUnsavedChangesChange: (Bool) -> Void
    @State private var query = ""
    @State private var completedQuery: String?
    @State private var showsAddMember = false
    @State private var pendingDisable: AdminOrganizationMemberRecord?
    @State private var errorMessage: String?

    var body: some View {
        Form {
            Section {
                ClassicSearchField(text: $query, prompt: String(localized: "Search members"), width: 300,
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
                    let isCurrentUser = member.id == workspaceContext.account?.userId
                    let ownerIsLocked = workspaceContext.account?.role != AdminOrganizationRole.owner.rawValue
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
                            Text(member.role.title + (isCurrentUser ? String(localized: " · You") : ""))
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
                                    mutate { try await administration.updateAdminOrganizationMember(member, status: .active) }
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
                    AdministrationLoadMore(section: .members, query: query)
                }
                Button("Add Member…") { showsAddMember = true }
                    .disabled(!allowsMutation)
            }
        }
        .formStyle(.grouped)
        .pageFeedback(state.errorMessage, isStatus: true) {
            Task { await administration.load(section: .members, force: true, query: query) }
        }
        .pageFeedback(errorMessage)
        .onChange(of: query) { _, _ in
            completedQuery = nil
            errorMessage = nil
        }
        .task(id: query) {
            let requestedQuery = query
            do {
                try await Task.sleep(for: .milliseconds(200))
                await administration.load(section: .members, query: requestedQuery)
                try Task.checkCancellation()
                if requestedQuery == query { completedQuery = requestedQuery }
            } catch {}
        }
        .sheet(isPresented: $showsAddMember) {
            AdministrationAddMemberSheet(onUnsavedChangesChange: onUnsavedChangesChange)
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
                mutate { try await administration.disableAdminOrganizationMember(member) }
                pendingDisable = nil
            }
        } message: { member in
            Text("This disables \(member.email) and revokes their active sessions.")
        }
    }

    private var assignableRoles: [AdminOrganizationRole] {
        var roles: [AdminOrganizationRole] = [.member, .admin]
        if workspaceContext.account?.role == AdminOrganizationRole.owner.rawValue {
            roles.append(.owner)
        }
        return roles
    }

    private var state: AdministrationPageState { administration.state(for: .members) }
    private var members: [AdminOrganizationMemberRecord] { administration.snapshot?.members ?? [] }
    private var allowsMutation: Bool {
        completedQuery == query && administration.canMutate(.members)
    }

    private func roleBinding(for member: AdminOrganizationMemberRecord) -> Binding<AdminOrganizationRole> {
        Binding(
            get: { member.role },
            set: { role in
                guard role != member.role else { return }
                mutate { try await administration.updateAdminOrganizationMember(member, role: role) }
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
                errorMessage = error.actionMessage
            }
        }
    }
}

private struct AdministrationAddMemberSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
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
                    VStack(alignment: .leading, spacing: 8) {
                        Text("This person can sign in with this email using your organization's single sign-on. No invitation email is sent.")
                        FormErrorMessage(message: errorMessage)
                    }
                }
                .disabled(workspaceContext.isMutatingAdministration)
            }
            .formStyle(.grouped)

            SheetActionBar(
                confirmationTitle: Text("Add"), progressTitle: "Adding…",
                isWorking: workspaceContext.isMutatingAdministration, canConfirm: canAdd,
                cancel: { dismiss() }, confirm: add
            )
        }
        .frame(width: 460, height: 285)
        .interactiveDismissDisabled(workspaceContext.isMutatingAdministration)
        .onChange(of: email.isEmpty) { _, empty in onUnsavedChangesChange(!empty) }
        .onDisappear { onUnsavedChangesChange(false) }
    }

    private var canAdd: Bool {
        administration.canMutate(.members)
            && email.contains("@")
            && !email.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var assignableRoles: [AdminOrganizationRole] {
        var roles: [AdminOrganizationRole] = [.member, .admin]
        if workspaceContext.account?.role == AdminOrganizationRole.owner.rawValue {
            roles.append(.owner)
        }
        return roles
    }

    private func add() {
        guard canAdd else { return }
        errorMessage = nil
        Task {
            do {
                try await administration.inviteAdminOrganizationMember(
                    email: email.trimmingCharacters(in: .whitespacesAndNewlines),
                    role: role
                )
                dismiss()
            } catch {
                errorMessage = error.actionMessage
            }
        }
    }
}

private struct AdministrationAccessView: View {
    @EnvironmentObject private var administration: AdministrationModel
    let snapshot: AdministrationSnapshot
    let onUnsavedChangesChange: (Bool) -> Void
    @State private var showsDomainEdit = false

    var body: some View {
        Form {
            Section("Sign-in") {
                if let provider = snapshot.identityProvider {
                    LabeledContent("Single sign-on", value: provider.configured ? String(localized: "Configured") : String(localized: "Not configured"))
                }
            }
            if let organization = snapshot.organization {
                Section {
                    LabeledContent("Allowed email domains") {
                        Text(organization.allowedEmailDomains.isEmpty
                            ? String(localized: "Any domain") : organization.allowedEmailDomains.joined(separator: ", "))
                            .fixedSize(horizontal: false, vertical: true)
                        Button("Edit…") { showsDomainEdit = true }
                            .disabled(!administration.canMutate(.organization))
                    }
                } footer: {
                    Text("People must be added as organization members before they can sign in.")
                }
                .sheet(isPresented: $showsDomainEdit) {
                    OrganizationEditSheet(organization: organization,
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
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
    @State private var query = ""
    @State private var completedQuery: String?

    var body: some View {
        Form {
            Section {
                ClassicSearchField(text: $query, prompt: String(localized: "Search activity"), width: 300,
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
                    AdministrationLoadMore(section: .audit, query: query)
                }
            }
        }
        .formStyle(.grouped)
        .pageFeedback(state.errorMessage, isStatus: true) {
            Task { await administration.load(section: .audit, force: true, query: query) }
        }
        .onChange(of: query) { _, _ in completedQuery = nil }
        .task(id: query) {
            let requestedQuery = query
            do {
                try await Task.sleep(for: .milliseconds(200))
                await administration.load(section: .audit, query: requestedQuery)
                try Task.checkCancellation()
                if requestedQuery == query { completedQuery = requestedQuery }
            } catch {}
        }
    }

    private var state: AdministrationPageState { administration.state(for: .audit) }
    private var events: [AdminAuditEventRecord] { administration.snapshot?.auditEvents ?? [] }

    private func actionTitle(_ action: String) -> String {
        switch action {
        case "admin.org_updated": String(localized: "Updated organization")
        case "admin.member_created": String(localized: "Added member")
        case "admin.member_updated": String(localized: "Updated member")
        case "admin.project_created": String(localized: "Created project")
        case "admin.project_updated": String(localized: "Updated project")
        case "admin.project_deleted": String(localized: "Deleted project")
        case "admin.project_member_created": String(localized: "Added project member")
        case "admin.project_member_updated": String(localized: "Updated project member")
        case "admin.project_member_deleted": String(localized: "Removed project member")
        case "admin.token_revoked": String(localized: "Revoked sign-in credential")
        default: String(localized: "Organization activity")
        }
    }

    private func targetName(_ event: AdminAuditEventRecord) -> String {
        if let name = event.targetDisplayName, !name.isEmpty { return name }
        switch event.targetType {
        case "org": return workspaceContext.organization?.name ?? String(localized: "Unavailable organization")
        case "user": return String(localized: "Unavailable member")
        case "project": return String(localized: "Unavailable project")
        case "project_member": return String(localized: "Unavailable project member")
        case "access_token": return String(localized: "Unavailable sign-in credential")
        default: return String(localized: "Unavailable item")
        }
    }

    private func actorName(_ event: AdminAuditEventRecord) -> String {
        if let name = event.actorDisplayName ?? event.actorEmail { return name }
        return event.actorUserId == nil ? String(localized: "System") : String(localized: "Unavailable member")
    }
}

private struct AdministrationLoadMore: View {
    @EnvironmentObject private var administration: AdministrationModel
    let section: AdministrationSection
    var query: String? = nil

    var body: some View {
        let state = administration.state(for: section)
        Group {
            if state.nextCursor != nil {
                HStack {
                    Button("Show More") {
                        Task { await administration.load(section: section, loadMore: true, query: query) }
                    }
                    .disabled(state.isLoading)
                    if state.isLoading { ProgressView().controlSize(.small) }
                    Spacer()
                }
                .padding(.vertical, 4)
            }
        }

    }
}
