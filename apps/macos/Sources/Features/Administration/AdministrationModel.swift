import Combine
import Foundation

@MainActor
final class AdministrationModel: ObservableObject {
    @Published private(set) var snapshot: AdministrationSnapshot?
    @Published private(set) var projectMembers: [String: [ProjectMemberRecord]] = [:]
    @Published private(set) var projectDetails: [String: AdminProjectRecord] = [:]
    @Published private(set) var projectDetailStates: [String: AdministrationPageState] = [:]
    @Published private(set) var pageStates: [AdministrationSection: AdministrationPageState] = [:]
    @Published private(set) var refreshGeneration = UUID()
    @Published private(set) var loadingProjectIds: Set<String> = []
    private var loadGenerations: [AdministrationSection: UUID] = [:]
    private var loadTasks: [AdministrationSection: Task<Void, Never>] = [:]
    private var projectMemberLoadGenerations: [String: UUID] = [:]
    private var projectDetailLoadGenerations: [String: UUID] = [:]

    private let context: WorkspaceContext
    private let onWorkspaceChanged: () async -> Void
    private let server: ServerClient
    private let fetchPage: @Sendable (String, [URLQueryItem]) async throws -> DaemonServerResponse
    private var observations: Set<AnyCancellable> = []

    init(
        context: WorkspaceContext,
        onWorkspaceChanged: @escaping () async -> Void,
        fetchPage: (@Sendable (String, [URLQueryItem]) async throws -> DaemonServerResponse)? = nil
    ) {
        self.context = context
        self.onWorkspaceChanged = onWorkspaceChanged
        let server = context.server
        self.server = server
        self.fetchPage = fetchPage ?? { path, query in
            try await server.raw(method: "GET", path: path, query: query)
        }
        context.$authorityGeneration.dropFirst().sink { [weak self] _ in
            self?.reset()
        }.store(in: &observations)
        context.$projects.sink { [weak self] projects in
            guard let self, !self.context.canAdministerOrganization else { return }
            let accessible = Set(projects.map(\.id))
            projectDetails = projectDetails.filter { accessible.contains($0.key) }
            projectMembers = projectMembers.filter { accessible.contains($0.key) }
            projectDetailStates = projectDetailStates.filter { accessible.contains($0.key) }
        }.store(in: &observations)
        context.projectDirectoryChanges.sink { [weak self] in
            self?.pageStates[.projects, default: .init()].isStale = true
        }.store(in: &observations)
    }

    func canMutateProject(_ projectId: String) -> Bool {
        let state = projectDetailStates[projectId]
        return context.phase == .ready && context.canManageProject(projectId)
            && state?.isLoaded == true && state?.isStale == false && state?.isLoading == false
            && !context.isMutatingAdministration
    }

    func state(for section: AdministrationSection) -> AdministrationPageState {
        pageStates[section] ?? AdministrationPageState()
    }

    func canMutate(_ section: AdministrationSection) -> Bool {
        let state = state(for: section)
        return Self.administrationMutationAllowed(
            capabilities: context.capabilities,
            phase: context.phase,
            hasSnapshot: state.isLoaded,
            isStale: state.isStale
        ) && !state.isLoading && loadTasks[section] == nil && !context.isMutatingAdministration
    }

    nonisolated static func administrationMutationAllowed(
        capabilities: Set<String>,
        phase: ApplicationPhase = .ready,
        hasSnapshot: Bool,
        isStale: Bool
    ) -> Bool {
        phase == .ready && capabilities.contains("admin:write") && hasSnapshot && !isStale
    }

    func load(
        section: AdministrationSection,
        force: Bool = false,
        loadMore: Bool = false,
        query: String? = nil
    ) async {
        guard !Task.isCancelled else { return }
        guard context.canAdministerOrganization, context.phase != .authenticationRequired else { return }
        var previous = state(for: section)
        let nextQuery = (query ?? previous.query).trimmingCharacters(in: .whitespacesAndNewlines)
        let queryChanged = previous.query != nextQuery
        if queryChanged {
            loadTasks[section]?.cancel()
            loadTasks[section] = nil
            loadGenerations[section] = UUID()
            previous = AdministrationPageState(query: nextQuery)
            pageStates[section] = previous
            if section == .members { snapshot?.members = [] }
            if section == .audit { snapshot?.auditEvents = [] }
        }
        if let task = loadTasks[section] {
            await task.value
            return
        }
        let appending = loadMore && !queryChanged
        guard force || appending || !previous.isLoaded || previous.isStale else { return }
        if appending, previous.nextCursor == nil { return }

        let task = Task {
            await performLoad(section: section, previous: previous, loadMore: appending)
        }
        loadTasks[section] = task
        await task.value
    }

    private func performLoad(
        section: AdministrationSection,
        previous: AdministrationPageState,
        loadMore: Bool
    ) async {
        guard !Task.isCancelled, context.canAdministerOrganization, context.phase != .authenticationRequired else { return }
        let generation = UUID()
        loadGenerations[section] = generation
        pageStates[section, default: .init()].isLoading = true
        pageStates[section, default: .init()].errorMessage = nil
        defer {
            if loadGenerations[section] == generation {
                pageStates[section, default: .init()].isLoading = false
                loadTasks[section] = nil
            }
        }

        do {
            let page = try await Self.loadAdministrationPage(
                section: section,
                cursor: loadMore ? previous.nextCursor : nil,
                seenCursors: loadMore ? previous.seenCursors : [],
                query: previous.query,
                request: fetchPage
            )
            try Task.checkCancellation()
            guard loadGenerations[section] == generation,
                  context.canAdministerOrganization, context.phase != .authenticationRequired else { return }
            var snapshot = self.snapshot ?? AdministrationSnapshot()
            snapshot.apply(page.snapshot, section: section, appending: loadMore)
            self.snapshot = snapshot
            pageStates[section] = AdministrationPageState(
                isLoaded: true,
                isLoading: true,
                isStale: page.isStale || (loadMore && previous.isStale),
                nextCursor: page.nextCursor,
                seenCursors: loadMore
                    ? previous.seenCursors.union(previous.nextCursor.map { [$0] } ?? [])
                    : [],
                query: previous.query
            )
            if section == .access, loadTasks[.organization] == nil {
                pageStates[.organization] = AdministrationPageState(isLoaded: true, isStale: page.isStale)
            }
            if section == .projects, !loadMore {
                projectMemberLoadGenerations.removeAll()
                projectMembers.removeAll()
                projectDetails.removeAll()
                projectDetailStates.removeAll()
                projectDetailLoadGenerations.removeAll()
                loadingProjectIds.removeAll()
                refreshGeneration = UUID()
            }
        } catch is CancellationError {
            return
        } catch {
            guard loadGenerations[section] == generation else { return }
            pageStates[section, default: .init()].isStale = true
            pageStates[section, default: .init()].errorMessage = error.localizedDescription
        }
    }

    nonisolated static func loadAdministrationPage(
        section: AdministrationSection,
        cursor: String? = nil,
        seenCursors: Set<String> = [],
        query searchQuery: String? = nil,
        request: @Sendable (String, [URLQueryItem]) async throws -> DaemonServerResponse
    ) async throws -> AdministrationPageResult {
        var result = AdministrationPageResult()
        var query = [URLQueryItem(name: "limit", value: "100")]
        if let cursor { query.append(URLQueryItem(name: "cursor", value: cursor)) }
        if section == .members || section == .audit,
           let searchQuery = searchQuery?.trimmingCharacters(in: .whitespacesAndNewlines), !searchQuery.isEmpty {
            query.append(URLQueryItem(name: "q", value: searchQuery))
        }

        func fetch<Value: Decodable & Sendable>(
            _ path: String,
            query: [URLQueryItem] = []
        ) async throws -> (Value, Bool) {
            let response = try await request(path, query)
            try Task.checkCancellation()
            guard (200..<300).contains(response.status) else {
                throw ServerClientError.response(status: response.status, message: response.body)
            }
            do {
                return (
                    try JSONCoding.decoder().decode(Value.self, from: Data(response.body.utf8)),
                    response.isStaleCache
                )
            } catch {
                throw ServerClientError.invalidResponse(ServerClient.decodingFailureMessage(
                    error, method: "GET", path: path, responseType: Value.self
                ))
            }
        }

        func list<Item: Decodable & Sendable>(
            _ path: String
        ) async throws -> ([Item], Bool, String?) {
            let (page, stale): (ListResponse<Item>, Bool) = try await fetch(path, query: query)
            let next = page.pageInfo.hasMore ? page.pageInfo.nextCursor : nil
            if page.pageInfo.hasMore {
                guard let next, !next.isEmpty, next != cursor, !seenCursors.contains(next) else {
                    throw ServerClientError.invalidResponse(String(localized: "Organization pagination returned an invalid next cursor."))
                }
            }
            return (page.items, stale, next)
        }

        switch section {
        case .organization:
            let (organization, stale): (AdminOrganizationRecord, Bool) =
                try await fetch("/api/v1/admin/org")
            result.snapshot.organization = organization
            result.isStale = stale
        case .members:
            (result.snapshot.members, result.isStale, result.nextCursor) =
                try await list("/api/v1/admin/members")
        case .projects:
            (result.snapshot.projects, result.isStale, result.nextCursor) =
                try await list("/api/v1/admin/projects")
        case .access:
            let (organization, organizationStale): (AdminOrganizationRecord, Bool) =
                try await fetch("/api/v1/admin/org")
            let (provider, providerStale): (AdminIdentityProviderStatus, Bool) =
                try await fetch("/api/v1/admin/identity-provider")
            result.snapshot.organization = organization
            result.snapshot.identityProvider = provider
            result.isStale = organizationStale || providerStale
        case .audit:
            (result.snapshot.auditEvents, result.isStale, result.nextCursor) =
                try await list("/api/v1/admin/audit-events")
        }
        return result
    }

    func searchProjectMemberCandidates(
        projectId: String, query: String, cursor: String? = nil
    ) async throws -> ListResponse<UserReference> {
        guard context.canManageProject(projectId) else { throw AdministrationError.forbidden }
        guard context.phase == .ready else { throw AdministrationError.unavailable }
        let generation = context.workspaceReloadGeneration
        var parameters = [URLQueryItem(name: "q", value: query), URLQueryItem(name: "limit", value: "50")]
        if let cursor { parameters.append(URLQueryItem(name: "cursor", value: cursor)) }
        let result: (value: ListResponse<UserReference>, response: DaemonServerResponse) =
            try await server.getWithMetadata("/api/v1/admin/projects/\(projectId)/member-candidates", query: parameters)
        try Task.checkCancellation()
        guard generation == context.workspaceReloadGeneration, context.phase == .ready, context.canManageProject(projectId) else {
            throw CancellationError()
        }
        guard !result.response.isStaleCache else { throw AdministrationError.stale }
        if result.value.pageInfo.hasMore {
            guard let next = result.value.pageInfo.nextCursor, !next.isEmpty, next != cursor else {
                throw ServerClientError.invalidResponse(String(localized: "Member pagination returned an invalid next cursor."))
            }
        }
        return result.value
    }

    func project(id: String) -> AdminProjectRecord? {
        projectDetails[id] ?? snapshot?.projects.first(where: { $0.id == id })
    }

    func loadProject(id: String, force: Bool = false) async {
        guard context.canAccessProjectSettings(id), context.phase != .authenticationRequired else { return }
        let previous = projectDetailStates[id] ?? AdministrationPageState()
        guard !previous.isLoading, force || project(id: id) == nil else { return }
        let generation = UUID()
        let requestedRefreshGeneration = refreshGeneration
        projectDetailLoadGenerations[id] = generation
        projectDetailStates[id, default: .init()].isLoading = true
        projectDetailStates[id, default: .init()].errorMessage = nil
        defer {
            if projectDetailLoadGenerations[id] == generation {
                projectDetailStates[id, default: .init()].isLoading = false
                projectDetailLoadGenerations[id] = nil
            }
        }
        do {
            let client = server
            let result = try await Self.fetchAdministrationProject(id: id) { path in
                try await client.raw(method: "GET", path: path)
            }
            guard projectDetailLoadGenerations[id] == generation,
                  refreshGeneration == requestedRefreshGeneration,
                  context.canAccessProjectSettings(id), context.phase != .authenticationRequired else { return }
            projectDetails[id] = result.project
            projectDetailStates[id] = AdministrationPageState(
                isLoaded: true, isLoading: true, isStale: result.isStale
            )
            await loadProjectMembers(projectId: id)
        } catch is CancellationError {
            return
        } catch {
            guard projectDetailLoadGenerations[id] == generation,
                  refreshGeneration == requestedRefreshGeneration,
                  context.canAccessProjectSettings(id), context.phase != .authenticationRequired else { return }
            projectDetailStates[id, default: .init()].isStale = true
            projectDetailStates[id, default: .init()].errorMessage = error.localizedDescription
        }
    }

    nonisolated static func fetchAdministrationProject(
        id: String,
        request: @Sendable (String) async throws -> DaemonServerResponse
    ) async throws -> (project: AdminProjectRecord, isStale: Bool) {
        let response = try await request("/api/v1/admin/projects/\(id)")
        try Task.checkCancellation()
        guard (200..<300).contains(response.status) else {
            throw ServerClientError.response(status: response.status, message: response.status == 404
                ? String(localized: "This project no longer exists or is no longer accessible.") : response.body)
        }
        let project = try JSONCoding.decoder().decode(AdminProjectRecord.self, from: Data(response.body.utf8))
        guard project.id == id else {
            throw ServerClientError.invalidResponse(String(localized: "The project response did not match the requested project."))
        }
        return (project, response.isStaleCache)
    }

    func loadProjectMembers(projectId: String) async {
        guard context.canAccessProjectSettings(projectId),
              project(id: projectId) != nil else {
            projectMembers[projectId] = nil
            return
        }
        guard !loadingProjectIds.contains(projectId) else { return }

        let generation = UUID()
        let requestedRefreshGeneration = refreshGeneration
        projectMemberLoadGenerations[projectId] = generation
        loadingProjectIds.insert(projectId)
        defer {
            if projectMemberLoadGenerations[projectId] == generation {
                projectMemberLoadGenerations[projectId] = nil
                loadingProjectIds.remove(projectId)
            }
        }
        do {
            let members: (
                items: [ProjectMemberRecord],
                hasStaleServerResponse: Bool
            ) = try await loadAllAdministrationItems(
                "/api/v1/admin/projects/\(projectId)/members"
            )
            try Task.checkCancellation()
            guard projectMemberLoadGenerations[projectId] == generation,
                  refreshGeneration == requestedRefreshGeneration,
                  context.canAccessProjectSettings(projectId) else { return }
            projectMembers[projectId] = members.items
            if members.hasStaleServerResponse {
                projectDetailStates[projectId, default: .init()].isStale = true
            }
        } catch is CancellationError {
            return
        } catch {
            guard projectMemberLoadGenerations[projectId] == generation,
                  refreshGeneration == requestedRefreshGeneration else { return }
            projectMembers[projectId] = nil
            projectDetailStates[projectId, default: .init()].isStale = true
            projectDetailStates[projectId, default: .init()].errorMessage = error.localizedDescription
        }
    }

    @discardableResult
    func updateAdminOrganization(
        name: String,
        allowedEmailDomains: [String],
        expectedRevision: Int
    ) async throws -> AdminOrganizationRecord {
        let generation = try beginAdministrationMutation(.organization)
        defer { context.finishAdministrationMutation(generation) }
        let updated: AdminOrganizationRecord = try await server.send(
            method: "PATCH",
            path: "/api/v1/admin/org",
            headers: ["If-Match": String(expectedRevision)],
            body: UpdateAdminOrganizationRequest(
                name: name,
                allowedEmailDomains: allowedEmailDomains
            )
        )
        try context.ensureCurrentAdministrationMutation(generation)
        context.updateOrganization(OrganizationReference(orgId: updated.orgId, name: updated.name))
        try await refreshAfterAdministrationMutation(
            generation: generation,
            section: .organization,
            invalidating: [.organization, .audit]
        )
        return updated
    }

    @discardableResult
    func inviteAdminOrganizationMember(
        email: String,
        role: AdminOrganizationRole
    ) async throws -> AdminOrganizationMemberRecord {
        let generation = try beginAdministrationMutation(.members)
        defer { context.finishAdministrationMutation(generation) }
        let member: AdminOrganizationMemberRecord = try await server.send(
            method: "POST",
            path: "/api/v1/admin/members",
            body: CreateAdminOrganizationMemberRequest(email: email, role: role)
        )
        try context.ensureCurrentAdministrationMutation(generation)
        try await refreshAfterAdministrationMutation(
            generation: generation,
            section: .members,
            invalidating: [.members, .audit]
        )
        return member
    }

    @discardableResult
    func updateAdminOrganizationMember(
        _ member: AdminOrganizationMemberRecord,
        role: AdminOrganizationRole? = nil,
        status: AdminMemberStatus? = nil
    ) async throws -> AdminOrganizationMemberRecord {
        let generation = try beginAdministrationMutation(.members)
        defer { context.finishAdministrationMutation(generation) }
        let updated: AdminOrganizationMemberRecord = try await server.send(
            method: "PATCH",
            path: "/api/v1/admin/members/\(member.id)",
            headers: ["If-Match": String(member.revision)],
            body: UpdateAdminOrganizationMemberRequest(role: role, status: status)
        )
        try context.ensureCurrentAdministrationMutation(generation)
        try await refreshAfterAdministrationMutation(
            generation: generation,
            section: .members,
            invalidating: [.members, .audit],
            refreshesWorkspace: member.id == context.account?.userId
        )
        return updated
    }

    func disableAdminOrganizationMember(_ member: AdminOrganizationMemberRecord) async throws {
        let generation = try beginAdministrationMutation(.members)
        defer { context.finishAdministrationMutation(generation) }
        let _: DeleteResult = try await server.send(
            method: "DELETE",
            path: "/api/v1/admin/members/\(member.id)",
            headers: ["If-Match": String(member.revision)],
            body: EmptyPayload()
        )
        try context.ensureCurrentAdministrationMutation(generation)
        try await refreshAfterAdministrationMutation(
            generation: generation,
            section: .members,
            invalidating: [.members, .audit],
            refreshesWorkspace: member.id == context.account?.userId
        )
    }

    @discardableResult
    func updateAdminProject(
        _ project: AdminProjectRecord,
        name: String,
        description: String
    ) async throws -> AdminProjectRecord {
        let generation = try beginAdministrationMutation(.projects, projectId: project.id)
        defer { context.finishAdministrationMutation(generation) }
        let updated: AdminProjectRecord = try await server.send(
            method: "PATCH",
            path: "/api/v1/admin/projects/\(project.id)",
            headers: ["If-Match": String(project.revision)],
            body: UpdateProjectRequest(name: name, description: description)
        )
        try context.ensureCurrentAdministrationMutation(generation)
        let isInDirectory = snapshot?.projects.contains(where: { $0.id == updated.id }) == true
        projectDetails[updated.id] = updated
        if isInDirectory { snapshot?.updateProject(updated) }
        try await refreshAfterAdministrationMutation(
            generation: generation,
            section: .projects,
            invalidating: [.audit],
            refreshesWorkspace: true,
            refreshesPage: !isInDirectory
        )
        return updated
    }

    func deleteAdminProject(_ project: AdminProjectRecord, onDeleted: () -> Void = {}) async throws {
        let generation = try beginAdministrationMutation(.projects, projectId: project.id)
        defer { context.finishAdministrationMutation(generation) }
        let _: DeleteResult = try await server.send(
            method: "DELETE",
            path: "/api/v1/admin/projects/\(project.id)",
            headers: ["If-Match": String(project.revision)],
            body: EmptyPayload()
        )
        try context.ensureCurrentAdministrationMutation(generation)
        context.removeProjectRole(project.id)
        let isInDirectory = snapshot?.projects.contains(where: { $0.id == project.id }) == true
        snapshot?.projects.removeAll { $0.id == project.id }
        projectDetails[project.id] = nil
        projectDetailStates[project.id] = nil
        projectDetailLoadGenerations[project.id] = nil
        projectMembers[project.id] = nil
        projectMemberLoadGenerations[project.id] = nil
        loadingProjectIds.remove(project.id)
        if isInDirectory {
            pageStates[.projects, default: .init()].offsetProjectCursor(by: -1)
        }
        onDeleted()
        try await refreshAfterAdministrationMutation(
            generation: generation,
            section: .projects,
            invalidating: [.audit],
            refreshesWorkspace: true,
            refreshesPage: false
        )
    }

    @discardableResult
    func addAdminProjectMember(
        projectId: String,
        userId: String,
        role: ProjectMemberRole
    ) async throws -> ProjectMemberRecord {
        let generation = try beginAdministrationMutation(.projects, projectId: projectId)
        defer { context.finishAdministrationMutation(generation) }
        let member: ProjectMemberRecord = try await server.send(
            method: "POST",
            path: "/api/v1/admin/projects/\(projectId)/members",
            body: CreateProjectMemberRequest(userId: userId, role: role)
        )
        try context.ensureCurrentAdministrationMutation(generation)
        try await refreshAfterAdministrationMutation(
            generation: generation,
            section: .projects,
            invalidating: [.audit],
            refreshesWorkspace: true,
            refreshesPage: false
        )
        try await refreshAdminProjectAfterMemberMutation(projectId: projectId, generation: generation)
        return member
    }

    func deleteAdminProjectMember(projectId: String, userId: String) async throws {
        let generation = try beginAdministrationMutation(.projects, projectId: projectId)
        defer { context.finishAdministrationMutation(generation) }
        let _: DeleteResult = try await server.send(
            method: "DELETE",
            path: "/api/v1/admin/projects/\(projectId)/members/\(userId)",
            body: EmptyPayload()
        )
        try context.ensureCurrentAdministrationMutation(generation)
        try await refreshAfterAdministrationMutation(
            generation: generation,
            section: .projects,
            invalidating: [.audit],
            refreshesWorkspace: true,
            refreshesPage: false
        )
        try await refreshAdminProjectAfterMemberMutation(projectId: projectId, generation: generation)
    }

    private func refreshAdminProjectAfterMemberMutation(projectId: String, generation: UUID) async throws {
        do {
            let result: (value: AdminProjectRecord, response: DaemonServerResponse) =
                try await server.getWithMetadata("/api/v1/admin/projects/\(projectId)")
            try context.ensureCurrentAdministrationMutation(generation)
            projectDetails[projectId] = result.value
            if snapshot?.projects.contains(where: { $0.id == projectId }) == true {
                snapshot?.updateProject(result.value)
            }
            if result.response.isStaleCache {
                projectDetailStates[projectId, default: .init()].isStale = true
            }
            await loadProjectMembers(projectId: projectId)
            try context.ensureCurrentAdministrationMutation(generation)
        } catch {
            try context.ensureCurrentAdministrationMutation(generation)
            projectDetailStates[projectId, default: .init()].isStale = true
            projectDetailStates[projectId, default: .init()].errorMessage = error.localizedDescription
            throw error
        }
    }

    private func beginAdministrationMutation(_ section: AdministrationSection, projectId: String? = nil) throws -> UUID {
        guard context.phase == .ready else { throw AdministrationError.unavailable }
        let state: AdministrationPageState
        if let projectId {
            guard context.canManageProject(projectId) else { throw AdministrationError.forbidden }
            state = projectDetailStates[projectId] ?? AdministrationPageState()
        } else {
            guard context.canAdministerOrganization else { throw AdministrationError.forbidden }
            state = self.state(for: section)
        }
        guard state.isLoaded else { throw AdministrationError.unavailable }
        guard !state.isStale else { throw AdministrationError.stale }
        guard !state.isLoading, !context.isMutatingAdministration else { throw AdministrationError.busy }
        pageStates[section, default: .init()].errorMessage = nil
        return try context.beginAdministrationMutation()
    }

    private func refreshAfterAdministrationMutation(
        generation: UUID,
        section: AdministrationSection,
        invalidating sections: Set<AdministrationSection>,
        refreshesWorkspace: Bool = false,
        refreshesPage: Bool = true
    ) async throws {
        try context.ensureCurrentAdministrationMutation(generation)
        for affected in sections {
            loadTasks[affected]?.cancel()
            loadTasks[affected] = nil
            loadGenerations[affected] = UUID()
            pageStates[affected, default: .init()].isLoading = false
            pageStates[affected, default: .init()].isStale = true
        }
        if refreshesWorkspace {
            await onWorkspaceChanged()
            try context.ensureCurrentAdministrationMutation(generation)
        }
        if refreshesPage, context.canAdministerOrganization {
            await load(section: section, force: true)
            try context.ensureCurrentAdministrationMutation(generation)
        }
    }

    private func loadAllAdministrationItems<Item: Decodable & Sendable>(
        _ path: String
    ) async throws -> (items: [Item], hasStaleServerResponse: Bool) {
        var items: [Item] = []
        var cursor: String?
        var seenCursors: Set<String> = []
        var hasStaleServerResponse = false
        repeat {
            var query = [URLQueryItem(name: "limit", value: "200")]
            if let cursor { query.append(URLQueryItem(name: "cursor", value: cursor)) }
            let page: (value: ListResponse<Item>, response: DaemonServerResponse) =
                try await server.getWithMetadata(path, query: query)
            try Task.checkCancellation()
            items += page.value.items
            hasStaleServerResponse = hasStaleServerResponse || page.response.isStaleCache
            if page.value.pageInfo.hasMore {
                guard let nextCursor = page.value.pageInfo.nextCursor,
                      !nextCursor.isEmpty,
                      nextCursor != cursor,
                      seenCursors.insert(nextCursor).inserted else {
                    throw ServerClientError.invalidResponse(
                        String(localized: "Administration pagination returned an invalid next cursor.")
                    )
                }
                cursor = nextCursor
            } else {
                cursor = nil
            }
        } while cursor != nil
        return (items, hasStaleServerResponse)
    }

    private func reset() {
        loadTasks.values.forEach { $0.cancel() }
        loadTasks.removeAll()
        loadGenerations.removeAll()
        projectMemberLoadGenerations.removeAll()
        snapshot = nil
        projectMembers.removeAll()
        projectDetails.removeAll()
        projectDetailStates.removeAll()
        projectDetailLoadGenerations.removeAll()
        pageStates.removeAll()
        refreshGeneration = UUID()
        loadingProjectIds.removeAll()
    }

}
