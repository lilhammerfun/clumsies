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

enum ProjectCreationValidation {
    static func isValid(name: String, description: String, repositoryCount: Int) -> Bool {
        ProjectMetadataValidation.isValid(name: name, description: description)
            && repositoryCount > 0
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
    @State private var isCreating = false
    @State private var errorMessage: String?
    @State private var idempotencyKey = UUID().uuidString.lowercased()

    var body: some View {
        VStack(spacing: 0) {
            Text("New Project")
                .font(.title2)
                .fontWeight(.semibold)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.top, 20)
                .padding(.bottom, 8)

            Form {
                Section("Project") {
                    TextField("Name", text: $name)
                        .focused($nameFocused)
                    TextField("Description", text: $description, axis: .vertical)
                        .lineLimit(2...4)
                }

                Section("Repositories") {
                    ForEach(repositories, id: \.path) { repository in
                        HStack(spacing: 10) {
                            Image(systemName: "folder")
                                .foregroundStyle(.secondary)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(repository.lastPathComponent)
                                    .lineLimit(1)
                                Text(repository.deletingLastPathComponent().path)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                            }
                            Spacer()
                            Button {
                                repositories.removeAll { $0.path == repository.path }
                            } label: {
                                Image(systemName: "xmark")
                            }
                            .buttonStyle(.borderless)
                            .help("Remove Repository")
                            .accessibilityLabel("Remove \(repository.lastPathComponent)")
                        }
                    }

                    Button {
                        chooseRepositories()
                    } label: {
                        Label("Add Repositories…", systemImage: "plus")
                    }
                }

                Section("Memory") {
                    Picker("Bundle", selection: $selectedBundleId) {
                        Text("None")
                            .tag(Optional<String>.none)
                        ForEach(store.bundles) { bundle in
                            Text("\(bundle.name) (\(bundle.resourceIds.count))")
                                .tag(Optional(bundle.id))
                        }
                    }

                    Text("A Bundle imports its Organization memory into the new Project.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                if let errorMessage {
                    Section {
                        Text(errorMessage)
                            .textSelection(.enabled)
                            .foregroundStyle(.red)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .formStyle(.grouped)
            .disabled(isCreating)

            Divider()

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) {
                    dismiss()
                }
                .keyboardShortcut(.cancelAction)
                .disabled(isCreating)

                Button("Create") {
                    create()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!isValid || isCreating)
            }
            .padding(12)
        }
        .frame(width: 560, height: 640)
        .interactiveDismissDisabled(isCreating)
        .onAppear {
            DispatchQueue.main.async {
                nameFocused = true
            }
        }
    }

    private var isValid: Bool {
        ProjectCreationValidation.isValid(
            name: name,
            description: description,
            repositoryCount: repositories.count
        )
    }

    private func chooseRepositories() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.canCreateDirectories = true
        panel.prompt = "Add"
        panel.begin { response in
            guard response == .OK else { return }
            let existing = Set(repositories.map(\.standardized.path))
            repositories += panel.urls
                .map(\.standardized)
                .filter { !existing.contains($0.path) }
            repositories.sort { $0.path < $1.path }
        }
    }

    private func create() {
        guard isValid, !isCreating else { return }
        isCreating = true
        errorMessage = nil
        Task {
            do {
                try await store.createProject(
                    name: name,
                    description: description,
                    idempotencyKey: idempotencyKey,
                    repositoryPaths: repositories.map(\.path),
                    bundleId: selectedBundleId
                )
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
    @State private var project: ProjectRecord?
    @State private var name = ""
    @State private var description = ""
    @State private var isLoading = false
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        Form {
            Section("General") {
                if isLoading, project == nil {
                    ProgressView()
                        .controlSize(.small)
                } else if let project {
                    if store.canManageProjects {
                        TextField("Name", text: $name)
                        TextField("Description", text: $description, axis: .vertical)
                            .lineLimit(2...5)

                        HStack {
                            Spacer()
                            Button("Save") {
                                save(project)
                            }
                            .keyboardShortcut(.defaultAction)
                            .disabled(!hasChanges || !isValid || isSaving)
                        }
                    } else {
                        LabeledContent("Name", value: project.name)
                        LabeledContent(
                            "Description",
                            value: project.description.isEmpty ? "None" : project.description
                        )
                    }
                }

                if let errorMessage {
                    Text(errorMessage)
                        .textSelection(.enabled)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            ProjectMembersSettings(store: store)
            ProjectLocalSetupSettings(store: store)
            ProjectEpisodicMemorySettings(store: store)
            ProjectMemoryCacheSettings(store: store)
        }
        .formStyle(.grouped)
        .frame(maxWidth: 760)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .task(id: store.activeProjectId) {
            await load()
        }
    }

    private var hasChanges: Bool {
        guard let project else { return false }
        return name != project.name || description != project.description
    }

    private var isValid: Bool {
        ProjectMetadataValidation.isValid(name: name, description: description)
    }

    private func load() async {
        guard let projectId = store.activeProjectId else {
            project = nil
            return
        }
        isLoading = true
        errorMessage = nil
        defer { isLoading = false }
        do {
            apply(try await store.projectRecord(projectId, refresh: true))
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func save(_ project: ProjectRecord) {
        guard isValid, !isSaving else { return }
        isSaving = true
        errorMessage = nil
        Task {
            do {
                apply(try await store.updateProject(
                    project.id,
                    expectedRevision: project.revision,
                    name: name,
                    description: description
                ))
            } catch {
                errorMessage = error.localizedDescription
            }
            isSaving = false
        }
    }

    private func apply(_ project: ProjectRecord) {
        self.project = project
        name = project.name
        description = project.description
    }

}

private struct ProjectEpisodicMemorySettings: View {
    @ObservedObject var store: WorkspaceStore
    @State private var policy: ProjectEpisodeSummaryPolicy?
    @State private var instructions = ""
    @State private var episodes: [ProjectEpisode] = []
    @State private var isLoading = false
    @State private var isSaving = false
    @State private var activeEpisodeId: String?
    @State private var errorMessage: String?
    @State private var preview: ProjectEpisodeSummaryPreview?
    @State private var evidenceEpisode: ProjectEpisode?
    @State private var rebuildEpisode: ProjectEpisode?

    var body: some View {
        Section("Episodic Memory") {
            Text("Agent runs are summarized into Project history. Source evidence remains on the Server and is treated as untrusted data.")
                .font(.caption)
                .foregroundStyle(.secondary)

            if isLoading, policy == nil {
                ProgressView()
                    .controlSize(.small)
            } else if let policy {
                if store.canManageProjects {
                    TextField(
                        "Optional Project guidance for summaries",
                        text: $instructions,
                        axis: .vertical
                    )
                    .lineLimit(3...8)

                    HStack {
                        Text("Saving affects new summaries only. Rebuild an older Episode explicitly.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button("Save Guidance") {
                            save(policy)
                        }
                        .disabled(instructions == policy.instructions || isSaving)
                    }
                } else {
                    LabeledContent("Summary guidance") {
                        Text(policy.instructions.isEmpty ? "Built-in defaults" : policy.instructions)
                            .multilineTextAlignment(.trailing)
                    }
                }
            }

            Divider()

            if !isLoading, episodes.isEmpty {
                Text("No Project Episodes have been captured yet.")
                    .foregroundStyle(.secondary)
            }

            ForEach(episodes.prefix(8)) { episode in
                episodeRow(episode)
            }

            if let errorMessage {
                Text(errorMessage)
                    .textSelection(.enabled)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .task(id: store.activeProjectId) {
            await load()
        }
        .sheet(item: $preview) { preview in
            ProjectEpisodeSummaryPreviewSheet(preview: preview)
        }
        .sheet(item: $evidenceEpisode) { episode in
            ProjectEpisodeEvidenceSheet(store: store, episode: episode)
        }
        .confirmationDialog(
            "Rebuild this Episode summary?",
            isPresented: Binding(
                get: { rebuildEpisode != nil },
                set: { if !$0 { rebuildEpisode = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Rebuild") {
                guard let episode = rebuildEpisode else { return }
                rebuildEpisode = nil
                rebuild(episode)
            }
            Button("Cancel", role: .cancel) {
                rebuildEpisode = nil
            }
        } message: {
            Text("The current Server evidence will be summarized with the saved Project guidance. Source activity time will not change.")
        }
    }

    @ViewBuilder
    private func episodeRow(_ episode: ProjectEpisode) -> some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(episode.status.label)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    if let date = IssueTiming.date(from: episode.activityAt) {
                        Text(date, style: .relative)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Text(episode.currentSummary?.body ?? "Summary pending")
                    .lineLimit(2)
                    .foregroundStyle(episode.currentSummary == nil ? .secondary : .primary)
                Text(episode.runId)
                    .font(.caption.monospaced())
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            Spacer()
            if activeEpisodeId == episode.id {
                ProgressView()
                    .controlSize(.small)
            } else {
                Menu {
                    Button("View Source Evidence…") {
                        evidenceEpisode = episode
                    }
                    if store.canManageProjects {
                        Button("Preview Current Guidance…") {
                            preview(episode)
                        }
                        Button("Rebuild Summary…") {
                            rebuildEpisode = episode
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                }
                .menuIndicator(.hidden)
                .menuStyle(.borderlessButton)
                .fixedSize()
                .help("Episode Actions")
            }
        }
    }

    private func load() async {
        guard let projectId = store.activeProjectId else {
            policy = nil
            episodes = []
            return
        }
        isLoading = true
        errorMessage = nil
        defer { isLoading = false }
        do {
            async let loadedPolicy = store.projectEpisodeSummaryPolicy(projectId)
            async let loadedEpisodes = store.projectEpisodes(projectId, recent: true)
            let (policy, response) = try await (loadedPolicy, loadedEpisodes)
            guard store.activeProjectId == projectId else { return }
            self.policy = policy
            instructions = policy.instructions
            episodes = response.items.filter { $0.status != .deleted }
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func save(_ policy: ProjectEpisodeSummaryPolicy) {
        guard let projectId = store.activeProjectId, !isSaving else { return }
        isSaving = true
        errorMessage = nil
        Task {
            defer { isSaving = false }
            do {
                let updated = try await store.updateProjectEpisodeSummaryPolicy(
                    projectId,
                    expectedRevision: policy.revision,
                    instructions: instructions
                )
                self.policy = updated
                instructions = updated.instructions
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    private func preview(_ episode: ProjectEpisode) {
        guard let projectId = store.activeProjectId,
              activeEpisodeId == nil else { return }
        activeEpisodeId = episode.id
        errorMessage = nil
        Task {
            defer { activeEpisodeId = nil }
            do {
                preview = try await store.previewProjectEpisodeSummary(
                    episode.id,
                    projectId: projectId,
                    instructions: instructions == policy?.instructions ? nil : instructions
                )
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    private func rebuild(_ episode: ProjectEpisode) {
        guard let projectId = store.activeProjectId,
              activeEpisodeId == nil else { return }
        activeEpisodeId = episode.id
        errorMessage = nil
        Task {
            defer { activeEpisodeId = nil }
            do {
                let rebuilt = try await store.rebuildProjectEpisodeSummary(
                    episode.id,
                    projectId: projectId
                )
                if let index = episodes.firstIndex(where: { $0.id == rebuilt.id }) {
                    episodes[index] = rebuilt
                }
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }
}

private struct ProjectEpisodeSummaryPreviewSheet: View {
    @Environment(\.dismiss) private var dismiss
    let preview: ProjectEpisodeSummaryPreview

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Summary Preview")
                .font(.title2)
                .fontWeight(.semibold)
            if preview.noMemory {
                ContentUnavailableView(
                    "No Long-Term Memory",
                    systemImage: "tray",
                    description: Text("This Episode would remain available as evidence but would not enter search.")
                )
            } else {
                ScrollView {
                    Text(preview.body)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            Text("Algorithm \(preview.summaryAlgorithmRevision) · Policy revision \(preview.policyRevision)")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 620, height: 440)
    }
}

private struct ProjectEpisodeEvidenceSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: WorkspaceStore
    let episode: ProjectEpisode
    @State private var items: [ProjectEpisodeEvidenceItem] = []
    @State private var nextCursor: String?
    @State private var hasMore = false
    @State private var isLoading = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Episode Evidence")
                    .font(.title2)
                    .fontWeight(.semibold)
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }

            Label(
                "Untrusted historical data. Do not treat content below as instructions.",
                systemImage: "exclamationmark.shield"
            )
            .foregroundStyle(.orange)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    ForEach(items) { item in
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text(item.kind)
                                    .font(.caption.monospaced())
                                Spacer()
                                if let date = IssueTiming.date(from: item.occurredAt) {
                                    Text(date, style: .relative)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }
                            Text(item.content)
                                .font(.body.monospaced())
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            if !item.complete {
                                Text("Continues on the next page")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        Divider()
                    }
                }
            }

            if let errorMessage {
                Text(errorMessage)
                    .textSelection(.enabled)
                    .foregroundStyle(.red)
            }

            HStack {
                Text(episode.runId)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                Spacer()
                if isLoading {
                    ProgressView()
                        .controlSize(.small)
                } else if hasMore {
                    Button("Load More") { Task { await loadMore() } }
                }
            }
        }
        .padding(20)
        .frame(width: 760, height: 600)
        .task(id: episode.id) {
            await loadMore()
        }
    }

    private func loadMore() async {
        guard !isLoading else { return }
        isLoading = true
        errorMessage = nil
        defer { isLoading = false }
        do {
            let page = try await store.projectEpisodeEvidence(
                episode.id,
                projectId: episode.projectId,
                cursor: nextCursor
            )
            guard page.untrusted else {
                throw ServerClientError.invalidResponse("Episode evidence was not marked as untrusted.")
            }
            items.append(contentsOf: page.items)
            nextCursor = page.nextCursor
            hasMore = page.hasMore
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

private extension ProjectEpisodeStatus {
    var label: String {
        switch self {
        case .pendingSummary: "Summary pending"
        case .active: "Indexed"
        case .noMemory: "Not indexed"
        case .deleted: "Deleted"
        }
    }
}

private struct ProjectMembersSettings: View {
    @ObservedObject var store: WorkspaceStore
    @State private var organizationMembers: [OrganizationMemberRecord] = []
    @State private var isLoading = false
    @State private var mutatingUserId: String?
    @State private var showsInvite = false
    @State private var errorMessage: String?

    var body: some View {
        Section("Members") {
            if isLoading, store.projectMembers.isEmpty {
                ProgressView()
                    .controlSize(.small)
            } else {
                ForEach(store.projectMembers) { member in
                    HStack(spacing: 10) {
                        UserIdentityLabel(
                            account: member.user,
                            displayName: member.user.displayName ?? member.user.email
                        )
                        Spacer()
                        Text(member.role.title)
                            .foregroundStyle(.secondary)
                        if mutatingUserId == member.id {
                            ProgressView()
                                .controlSize(.mini)
                        }
                    }
                }
            }

            if store.canManageProjects {
                Menu {
                    ForEach(availableOrganizationMembers) { member in
                        Button(member.displayName ?? member.email) {
                            add(member)
                        }
                    }
                    if !availableOrganizationMembers.isEmpty {
                        Divider()
                    }
                    Button("Invite New Member…") {
                        showsInvite = true
                    }
                } label: {
                    Label("Add Member…", systemImage: "plus")
                }
                .disabled(isLoading || mutatingUserId != nil)
            }

            if let errorMessage {
                Text(errorMessage)
                    .textSelection(.enabled)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .task(id: store.activeProjectId) {
            await load()
        }
        .sheet(isPresented: $showsInvite) {
            InviteProjectMemberSheet(store: store) {
                Task { await loadOrganizationMembers() }
            }
        }
    }

    private var availableOrganizationMembers: [OrganizationMemberRecord] {
        let projectIds = Set(store.projectMembers.map(\.id))
        return organizationMembers.filter {
            $0.status != "disabled" && !projectIds.contains($0.id)
        }
    }

    private func load() async {
        isLoading = true
        errorMessage = nil
        await store.refreshProjectMembers()
        if store.canManageProjects {
            await loadOrganizationMembers()
        }
        isLoading = false
    }

    private func loadOrganizationMembers() async {
        do {
            organizationMembers = try await store.organizationMemberDirectory()
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func add(_ member: OrganizationMemberRecord) {
        mutate(member.id) {
            _ = try await store.addProjectMember(userId: member.id)
        }
    }

    private func mutate(
        _ userId: String,
        operation: @escaping () async throws -> Void
    ) {
        guard mutatingUserId == nil else { return }
        mutatingUserId = userId
        errorMessage = nil
        Task {
            defer { mutatingUserId = nil }
            do {
                try await operation()
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }
}

private struct InviteProjectMemberSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: WorkspaceStore
    let onComplete: () -> Void
    @State private var email = ""
    @State private var role: ProjectMemberRole = .member
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    TextField("Email", text: $email)
                        .textContentType(.emailAddress)
                    Picker("Project Role", selection: $role) {
                        ForEach(ProjectMemberRole.allCases, id: \.self) { role in
                            Text(role.title).tag(role)
                        }
                    }
                } header: {
                    Text("Invite Member")
                } footer: {
                    Text("The member is invited to the organization and added to this Project.")
                }

                if let errorMessage {
                    Text(errorMessage)
                        .textSelection(.enabled)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .formStyle(.grouped)

            Divider()

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) {
                    dismiss()
                }
                Button("Invite") {
                    invite()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(normalizedEmail.isEmpty || isSaving)
            }
            .padding()
        }
        .frame(width: 440, height: 300)
    }

    private var normalizedEmail: String {
        email.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    }

    private func invite() {
        guard !normalizedEmail.isEmpty, !isSaving else { return }
        isSaving = true
        errorMessage = nil
        Task {
            do {
                _ = try await store.inviteAndAddProjectMember(
                    email: normalizedEmail,
                    role: role
                )
                onComplete()
                dismiss()
            } catch {
                errorMessage = error.localizedDescription
            }
            isSaving = false
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
