import Foundation

struct InboxNotification: Codable, Sendable {
    let notificationId: String
    let projectId: String
    let projectName: String
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
    case retrySync
}

enum InboxMessageType: String, CaseIterable, Identifiable, Sendable {
    case reviewRequests = "Review Requests"
    case reviewComments = "Review Comments"
    case reviewResults = "Review Results"
    case sharedUpdates = "Remote Updates"
    case syncErrors = "Sync Errors"

    var id: Self { self }

    var title: String {
        switch self {
        case .reviewRequests: String(localized: "Review Requests")
        case .reviewComments: String(localized: "Review Comments")
        case .reviewResults: String(localized: "Review Results")
        case .sharedUpdates: String(localized: "Remote Updates")
        case .syncErrors: String(localized: "Sync Errors")
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
    let destination: InboxDestination?
    var serverVersion: Int?

    var actionTitle: String? {
        guard let destination else { return nil }
        return switch destination {
        case .review: String(localized: "Open Review")
        case .sharedChanges: String(localized: "Open Memory")
        case .retrySync: String(localized: "Retry Sync")
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
        case "review_requested": .reviewRequests
        case "review_comment": .reviewComments
        case "shared_update": .sharedUpdates
        default: .reviewResults
        }
        let reason: String = switch notice.kind {
        case "review_requested": notice.reviewStatus == "open" ? String(localized: "Review requested") : updatedReviewReason(notice.reviewStatus)
        case "review_comment": String(localized: "New review comment")
        case "review_approved": String(localized: "Review approved")
        case "review_rejected": String(localized: "Changes requested")
        case "review_merged": String(localized: "Review merged")
        default: String(localized: "Remote Memory updated")
        }
        let shared = notice.kind == "shared_update"
        return .init(
            id: notice.notificationId, type: type, projectId: notice.projectId, projectName: notice.projectName,
            title: notice.title, message: [reason, notice.actorName].compactMap { $0 }.joined(separator: " · "),
            occurredAt: TimestampFormatting.date(from: notice.occurredAt) ?? .distantPast,
            needsAction: notice.needsAction, revision: String(notice.version),
            isRead: notice.readVersion >= notice.version, isArchived: notice.archivedVersion >= notice.version,
            destination: shared ? .sharedChanges(projectId: notice.projectId) : .review(notice.targetId),
            serverVersion: notice.version
        )
    }
}
