import Combine
import Foundation

@MainActor
final class WorkspaceContext: ObservableObject {
    let projectSelectionChanges = PassthroughSubject<Void, Never>()

    init() {
    }

    @Published var phase: ApplicationPhase = .launching
    @Published var account: UserReference?
    @Published var organization: OrganizationReference?
    @Published var capabilities: Set<String> = []
    @Published var projects: [ProjectState] = []
    @Published var projectRoles: [String: ProjectMemberRole] = [:]
    @Published var isMutatingAdministration = false
    @Published var authorityGeneration = UUID()

    let projectDirectoryChanges = PassthroughSubject<Void, Never>()

    @Published var activeProjectId: String? {
        didSet {
            if oldValue != activeProjectId { projectSelectionChanges.send() }
        }
    }

    @Published var loadingProjectId: String?
    @Published var isSwitchingMemoryContext = false

    let daemon = DaemonXPCClient()

    let bootstrap = DaemonBootstrapController()

    lazy var server = ServerClient(daemon: daemon)

    var workspaceReloadGeneration = UUID()

    var administrationMutationGeneration = UUID()

    var isSigningOut = false

    var projectSelectionGeneration = UUID()

    let projectSelectionSideEffectGate = ProjectSelectionSideEffectGate()

    var activeProject: ProjectState? {
        projects.first { $0.id == self.activeProjectId }
    }

    var canCreateProject: Bool {
        Self.projectCreationAllowed(capabilities: capabilities)
    }

    nonisolated static func projectCreationAllowed(capabilities: Set<String>) -> Bool {
        capabilities.contains("project:create") || capabilities.contains("admin:write")
    }

    func canManageProject(_ projectId: String) -> Bool {
        Self.projectManagementAllowed(capabilities: capabilities, role: projectRoles[projectId])
    }

    nonisolated static func projectManagementAllowed(
        capabilities: Set<String>, role: ProjectMemberRole?
    ) -> Bool {
        capabilities.contains("admin:write") || role == .owner || role == .admin
    }

    func canAccessProjectSettings(_ projectId: String) -> Bool {
        canAdministerOrganization || projects.contains { $0.id == projectId }
    }

    var canAdministerOrganization: Bool {
        capabilities.contains("admin:write")
    }

    var canMergeReviews: Bool {
        capabilities.contains("review:merge")
    }

    var canDecideReviews: Bool {
        capabilities.contains("review:decide")
    }

    func isReviewAuthor(_ review: ReviewRecord) -> Bool {
        account?.userId == review.author.userId
    }

    func ensureAuthority(_ generation: UUID) throws {
        try Task.checkCancellation()
        guard authorityGeneration == generation else { throw CancellationError() }
    }

    nonisolated static func projectContextIsCurrent(
        isSwitchingMemoryContext: Bool,
        activeProjectId: String?,
        expectedProjectId: String
    ) -> Bool {
        !isSwitchingMemoryContext && activeProjectId == expectedProjectId
    }

    func beginAdministrationMutation() throws -> UUID {
        guard phase == .ready else { throw AdministrationError.unavailable }
        guard !isMutatingAdministration else { throw AdministrationError.busy }
        let generation = UUID()
        administrationMutationGeneration = generation
        isMutatingAdministration = true
        return generation
    }

    func ensureCurrentAdministrationMutation(_ generation: UUID) throws {
        guard administrationMutationGeneration == generation,
              phase == .ready else {
            throw CancellationError()
        }
    }

    func finishAdministrationMutation(_ generation: UUID) {
        guard administrationMutationGeneration == generation else { return }
        isMutatingAdministration = false
    }

    func updateOrganization(_ organization: OrganizationReference) {
        self.organization = organization
    }

    func removeProjectRole(_ projectId: String) {
        projectRoles[projectId] = nil
    }

    func invalidateAdministrationAuthority() {
        administrationMutationGeneration = UUID()
        isMutatingAdministration = false
        authorityGeneration = UUID()
    }

    var loader: WorkspaceLoader {
        WorkspaceLoader(daemon: daemon, bootstrap: bootstrap, server: server)
    }

    func invalidateProjectSelection() {
        projectSelectionGeneration = UUID()
        loadingProjectId = nil
        isSwitchingMemoryContext = false
    }

    func resetAuthority() {
        ClientServiceStatus.shared.reset()
        workspaceReloadGeneration = UUID()
        invalidateProjectSelection()
        account = nil
        organization = nil
        capabilities.removeAll()
        projectRoles.removeAll()
        projects.removeAll()
        activeProjectId = nil
        invalidateAdministrationAuthority()
    }

    func applyAuthority(_ snapshot: WorkspaceSnapshot) {
        account = snapshot.account
        organization = snapshot.organization
        let lostOrganizationAuthority = canAdministerOrganization && !snapshot.capabilities.contains("admin:write")
        capabilities = snapshot.capabilities
        if lostOrganizationAuthority { invalidateAdministrationAuthority() }
        projects = snapshot.projects
        projectRoles = snapshot.projectRoles
        activeProjectId = snapshot.activeProjectId
    }
}

enum AdministrationError: UserFacingError, Sendable {
    case forbidden
    case unavailable
    case stale
    case busy

    var errorDescription: String? {
        switch self {
        case .forbidden:
            String(localized: "Organization administrator access is required.")
        case .unavailable:
            String(localized: "Load this organization page before making changes.")
        case .stale:
            String(localized: "This organization page is showing cached data. Refresh with a live Server connection before making changes.")
        case .busy:
            String(localized: "Another organization operation is still in progress.")
        }
    }
}
