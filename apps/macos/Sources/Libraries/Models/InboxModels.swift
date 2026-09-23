import Foundation

struct InboxNotification: Codable, Sendable {
    let notificationId: String
    let projectId: String?
    let projectName: String?
    let kind: String
    let targetId: String
    let title: String
    let actorName: String?
    let version: Int
    var readVersion: Int
    var archivedVersion: Int
    let needsAction: Bool
    let reviewStatus: String?
    let occurredAt: String
    let body: String?
    let previousRole: String?
    let newRole: String?
    let canOpenProject: Bool
}

struct InboxPage: Codable, Sendable {
    let items: [InboxNotification]
    let nextCursor: String?
}

struct InboxReceiptRequest: Codable, Sendable {
    let version: Int
    let action: InboxReceiptAction
}

enum InboxReceiptAction: String, Codable, Sendable {
    case read, unread, archive, restore
}

enum InboxDestination: Hashable, Sendable {
    case review(String)
    case sharedChanges(projectId: String)
    case project(String)
    case retrySync
    case manageLocalProjects
}

enum InboxMessageType: String, CaseIterable, Identifiable, Sendable {
    case reviewRequests = "Review Requests"
    case reviewComments = "Review Comments"
    case reviewResults = "Review Results"
    case sharedUpdates = "Remote Updates"
    case syncErrors = "Sync Errors"
    case welcome = "Welcome"
    case accessChanges = "Access Changes"

    var id: Self { self }

    var title: String {
        switch self {
        case .reviewRequests: String(localized: "Review Requests")
        case .reviewComments: String(localized: "Review Comments")
        case .reviewResults: String(localized: "Review Results")
        case .sharedUpdates: String(localized: "Remote Updates")
        case .syncErrors: String(localized: "Sync Errors")
        case .welcome: String(localized: "Welcome")
        case .accessChanges: String(localized: "Access Changes")
        }
    }
}

struct InboxItem: Identifiable, Sendable {
    let id: String
    let type: InboxMessageType
    let projectId: String?
    let projectName: String?
    let title: String
    let message: String
    let occurredAt: Date
    let needsAction: Bool
    let revision: String
    var isRead: Bool
    var isArchived: Bool
    var destination: InboxDestination?
    var serverVersion: Int?
    var body: String?

    var actionTitle: String? {
        if body != nil { return String(localized: "Read Message") }
        guard let destination else { return nil }
        return switch destination {
        case .review: String(localized: "Open Review")
        case .sharedChanges: String(localized: "Open Memory")
        case .retrySync: String(localized: "Retry Sync")
        case .manageLocalProjects: String(localized: "Manage Unavailable Projects")
        case .project: String(localized: "Open Project")
        }
    }

    var summary: String {
        [message, projectName == title ? nil : projectName].compactMap { $0 }.joined(separator: " · ")
    }

    private static func updatedReviewReason(_ status: String?) -> String {
        switch status {
        case "approved": String(localized: "Review approved")
        case "rejected": String(localized: "Changes requested")
        case "merged": String(localized: "Review merged")
        default: String(localized: "Review updated")
        }
    }

    static func server(_ notice: InboxNotification) -> Self {
        let type: InboxMessageType = switch notice.kind {
        case "welcome": .welcome
        case "project_joined", "project_removed", "project_role_changed", "org_role_changed": .accessChanges
        case "review_requested": .reviewRequests
        case "review_comment": .reviewComments
        case "shared_update": .sharedUpdates
        default: .reviewResults
        }
        let roleTitle: (String) -> String? = notice.kind == "org_role_changed"
            ? { AdminOrganizationRole(rawValue: $0)?.title }
            : { ProjectMemberRole(rawValue: $0)?.title }
        let previousRole = notice.previousRole.flatMap(roleTitle) ?? ""
        let newRole = notice.newRole.flatMap(roleTitle) ?? ""
        let actor = notice.actorName ?? String(localized: "An administrator")
        let reason: String = switch notice.kind {
        case "welcome": String(localized: "Get started with projects, Memory, and your team.")
        case "project_joined": String(localized: "\(actor) added you as \(newRole).")
        case "project_removed": String(localized: "\(actor) removed your access to this project.")
        case "project_role_changed": String(localized: "\(actor) changed your project role: \(previousRole) → \(newRole).")
        case "org_role_changed": String(localized: "\(actor) changed your organization role: \(previousRole) → \(newRole).")
        case "review_requested": notice.reviewStatus == "open" ? String(localized: "Review requested") : updatedReviewReason(notice.reviewStatus)
        case "review_comment": String(localized: "New review comment")
        case "review_approved": String(localized: "Review approved")
        case "review_rejected": String(localized: "Changes requested")
        case "review_merged": String(localized: "Review merged")
        default: String(localized: "Remote Memory updated")
        }
        let title: String = switch notice.kind {
        case "welcome": String(localized: "Welcome to Clumsies")
        case "project_joined": String(localized: "You've been added to a project")
        case "project_removed": String(localized: "Project access removed")
        case "project_role_changed": String(localized: "Project role changed")
        case "org_role_changed": String(localized: "Organization role changed")
        default: notice.title
        }
        let destination: InboxDestination? = switch notice.kind {
        case "welcome", "project_removed", "org_role_changed": nil
        case "project_joined", "project_role_changed": notice.canOpenProject ? notice.projectId.map(InboxDestination.project) : nil
        case "shared_update": notice.projectId.map { .sharedChanges(projectId: $0) }
        case "review_requested", "review_comment", "review_approved", "review_rejected", "review_merged": .review(notice.targetId)
        default: nil
        }
        return .init(
            id: notice.notificationId, type: type, projectId: notice.projectId, projectName: notice.projectName,
            title: title, message: type == .accessChanges || type == .welcome ? reason
                : [reason, notice.actorName].compactMap { $0 }.joined(separator: " · "),
            occurredAt: TimestampFormatting.date(from: notice.occurredAt) ?? .distantPast,
            needsAction: notice.needsAction, revision: String(notice.version),
            isRead: notice.readVersion >= notice.version, isArchived: notice.archivedVersion >= notice.version,
            destination: destination, serverVersion: notice.version,
            body: notice.kind == "welcome" ? notice.body.map { String(localized: String.LocalizationValue($0)) } : nil
        )
    }
}
