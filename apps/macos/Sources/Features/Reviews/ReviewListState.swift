import SwiftUI

enum ReviewStatusFilter: String, CaseIterable, Identifiable {
    case open
    case rejected
    case merged
    case all

    var id: String { rawValue }

    var title: String {
        switch self {
        case .open: "Open"
        case .rejected: "Rejected"
        case .merged: "Merged"
        case .all: "All"
        }
    }

    var symbolName: String {
        switch self {
        case .open: "clock"
        case .rejected: "xmark.circle"
        case .merged: "arrow.triangle.merge"
        case .all: "tray.full"
        }
    }

    func matches(_ review: ReviewRecord) -> Bool {
        self == .all || review.status == rawValue
    }

    func count(in reviews: [ReviewRecord]) -> Int {
        reviews.lazy.filter(matches).count
    }
}

struct ReviewListFilters: Equatable {
    var status: ReviewStatusFilter = .open
    var authorId: String? = nil
    var projectId: String? = nil

    func matches(_ review: ReviewRecord) -> Bool {
        status.matches(review)
            && (authorId == nil || review.author.userId == authorId)
            && (projectId == nil || review.projectId == projectId)
    }
}

struct ReviewRoute: Hashable {
    let reviewId: String
}

struct ReviewToolbarOwnership: Equatable {
    enum Surface: Equatable {
        case list
        case detail
    }

    enum Item: Equatable {
        case filter
        case decision(ReviewMenuAction)
        case search
    }

    let surface: Surface
    let items: [Item]

    static func resolve(
        surface: Surface,
        review: ReviewRecord?,
        canDecideReviews: Bool,
        canMergeReviews: Bool,
        isAuthor: Bool
    ) -> Self {
        guard surface == .detail else {
            return .init(surface: .list, items: [.filter, .search])
        }

        let actions = review.map { review in
            [
                ReviewMenuAction.reject,
                .approve,
                .merge,
                .resubmit,
            ].filter {
                $0.isAvailable(
                    for: review,
                    canDecideReviews: canDecideReviews,
                    canMergeReviews: canMergeReviews,
                    isAuthor: isAuthor
                )
            }
        } ?? []

        return .init(
            surface: .detail,
            items: actions.map(Item.decision)
        )
    }

    func contains(_ item: Item) -> Bool {
        items.contains(item)
    }

    var hasDecisionActions: Bool {
        items.contains {
            if case .decision = $0 { return true }
            return false
        }
    }
}

/// Reconciliation status shared by the Review queue and its file navigator.
enum ReviewReconciliationState: String, Hashable, Sendable {
    case conflict = "Conflict"
    case autoRebased = "Auto-rebased"
    case checking = "Checking…"

    var badgeColor: Color? {
        switch self {
        case .conflict: Color(nsColor: .systemRed)
        case .autoRebased: Color(nsColor: .systemPurple)
        case .checking: nil
        }
    }

    static func resolve(freshness: DraftFreshness, reconciliation: DraftReconciliationStatus,
                        autoRebased: Bool) -> Self? {
        if freshness == .behind { return reconciliation == .conflicts ? .conflict : .checking }
        return autoRebased ? .autoRebased : nil
    }
}
