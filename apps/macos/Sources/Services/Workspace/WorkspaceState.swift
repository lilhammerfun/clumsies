import Foundation

enum ApplicationPhase: Equatable, Sendable {
    case launching
    case authenticationRequired
    case loading
    case ready
    case failed(String)
}

enum WorkspaceCollectionLoadState: Equatable, Sendable {
    case loading
    case loaded
    case failed(String)

    var isLoading: Bool {
        self == .loading
    }

    var failureMessage: String? {
        guard case .failed(let message) = self else { return nil }
        return message
    }
}

struct WorkspaceSnapshot: Sendable {
    let account: UserReference
    let organization: OrganizationReference
    let capabilities: Set<String>
    let projects: [ProjectState]
    var projectRoles: [String: ProjectMemberRole] = [:]
    let activeProjectId: String?
    let orgRefCommitId: String?
    let orgRefEtag: String
    let resources: [MemoryResource]
    let runtime: RuntimeState
    let legacyAgentAdapterConflicts: [DaemonLegacyAgentAdapterConflict]
    let legacyAgentAdapterInspectionWarning: String?
}

enum WorkspaceLoadError: LocalizedError, Sendable {
    case authenticationRequired
    case noProjects
    case sharedStateChangedDuringLoad

    var errorDescription: String? {
        switch self {
        case .authenticationRequired: String(localized: "Sign in to connect Clumsies to your organization.")
        case .noProjects: String(localized: "The signed-in account has no accessible project.")
        case .sharedStateChangedDuringLoad:
            String(localized: "Remote memory changed while the workspace was loading. Refresh to load one consistent version.")
        }
    }
}

enum WorkspaceRefreshCadence {
    static let syncStatus: Duration = .seconds(2)
    static let synchronizedData: Duration = .seconds(30)
}
