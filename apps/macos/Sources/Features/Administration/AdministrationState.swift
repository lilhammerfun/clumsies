import Foundation

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

struct AdministrationSnapshot: Hashable, Sendable {
    var organization: AdminOrganizationRecord?
    var members: [AdminOrganizationMemberRecord] = []
    var projects: [AdminProjectRecord] = []
    var auditEvents: [AdminAuditEventRecord] = []
    var identityProvider: AdminIdentityProviderStatus?

    mutating func apply(
        _ page: AdministrationSnapshot,
        section: AdministrationSection,
        appending: Bool
    ) {
        switch section {
        case .organization: organization = page.organization
        case .members: Self.apply(page.members, to: &members, appending: appending)
        case .projects: Self.apply(page.projects, to: &projects, appending: appending)
        case .access:
            organization = page.organization
            identityProvider = page.identityProvider
        case .audit: Self.apply(page.auditEvents, to: &auditEvents, appending: appending)
        }
    }

    mutating func updateProject(_ project: AdminProjectRecord) {
        if let index = projects.firstIndex(where: { $0.id == project.id }) {
            projects[index] = project
        } else {
            projects.insert(project, at: 0)
        }
    }

    private static func apply<Item: Identifiable>(
        _ page: [Item],
        to items: inout [Item],
        appending: Bool
    ) {
        guard appending else { items = page; return }
        let existing = Set(items.map(\.id))
        items += page.filter { !existing.contains($0.id) }
    }
}

struct AdministrationPageState: Sendable {
    var isLoaded = false
    var isLoading = false
    var isStale = true
    var errorMessage: String?
    var nextCursor: String?
    var seenCursors: Set<String> = []
    var query = ""

    mutating func offsetProjectCursor(by delta: Int) {
        guard let nextCursor else { return }
        // Admin project cursors are decimal offsets. Local inserts/deletions move that boundary.
        guard let offset = Int(nextCursor), offset >= 0 else {
            isStale = true
            self.nextCursor = nil
            return
        }
        let (adjusted, overflow) = offset.addingReportingOverflow(delta)
        guard !overflow, adjusted >= 0 else {
            isStale = true
            self.nextCursor = nil
            return
        }
        self.nextCursor = String(adjusted)
        seenCursors.removeAll()
    }
}

struct AdministrationPageResult: Sendable {
    var snapshot = AdministrationSnapshot()
    var isStale = false
    var nextCursor: String?
}
