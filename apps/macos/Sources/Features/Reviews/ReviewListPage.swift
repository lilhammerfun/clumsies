import SwiftUI

struct ReviewStatusFilterControl: View {
    let reviews: [ReviewRecord]
    @Binding var selection: ReviewStatusFilter

    var body: some View {
        ToolbarFilterMenu(selectionTitle: selection.title) {
            ForEach(ReviewStatusFilter.allCases) { filter in
                Toggle(
                    label(for: filter),
                    isOn: Binding(
                        get: { selection == filter },
                        set: { isSelected in
                            guard isSelected else { return }
                            selection = filter
                        }
                    )
                )
            }
        }
        .toolbarHelp(String(localized: "Filter Reviews: \(label(for: selection))"))
        .accessibilityLabel("Filter Reviews")
        .accessibilityValue(label(for: selection))
        .accessibilityIdentifier("review-toolbar-filter")
    }

    private func label(for filter: ReviewStatusFilter) -> String {
        "\(filter.title) (\(filter.count(in: reviews)))"
    }
}

struct ReviewListPage: View {
    @Environment(\.workspaceActions) private var workspaceActions
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var reviewModel: ReviewsModel
    let reviews: [ReviewRecord]
    let searchQuery: String
    @Binding var filters: ReviewListFilters
    let toolbarOwnership: ReviewToolbarOwnership
    let onClearFilters: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Group {
                switch ReviewListContentState.resolve(
                    loadState: reviewModel.reviewLoadState,
                    totalCount: reviewModel.reviews.count,
                    visibleCount: reviews.count
                ) {
                case .loading:
                    ContentLoadingView(title: String(localized: "Loading Reviews…"))
                case .failed:
                    ContentUnavailableView {
                        Label("Reviews Unavailable", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(reviewModel.reviewLoadState.failureMessage ?? String(localized: "Reviews could not be loaded."))
                    } actions: {
                        Button("Try Again") { Task { await workspaceActions.reload() } }
                    }
                case .empty:
                    ContentUnavailableView(
                        "No Reviews",
                        systemImage: "checkmark.bubble",
                        description: Text("Reviews created from synchronized drafts appear here.")
                    )
                case .filteredEmpty:
                    if !trimmedSearchQuery.isEmpty {
                        ContentUnavailableView.search(text: trimmedSearchQuery)
                    } else {
                        ContentUnavailableView {
                            Label("No Matching Reviews", systemImage: "line.3.horizontal.decrease.circle")
                        } description: {
                            Text("No Reviews match the current filters.")
                        } actions: {
                            Button("Clear Filters", action: onClearFilters)
                        }
                    }
                case .content:
                    List {
                        ForEach(reviews) { review in
                            let route = ReviewRoute(reviewId: review.id)
                            let state = ReviewQueueStatePresentation.resolve(
                                review: review,
                                isAuthor: workspaceContext.isReviewAuthor(review),
                                canMerge: workspaceContext.canMergeReview(review)
                            )
                            NavigationLink(value: route) {
                                ReviewRow(
                                    review: review,
                                    projectName: filters.projectId == nil ? projectName(for: review) : nil,
                                    state: state,
                                    errorMessage: reviewModel.updates[review.id]?.errorMessage
                                )
                            }
                            .help(reviewModel.updates[review.id]?.errorMessage ?? review.title)
                            .task(id: review) { await reviewModel.prepareUpdate(review) }
                            .accessibilityIdentifier("review-row-\(review.id)")
                        }
                    }
                    .listStyle(.inset)
                    .pageFeedback(!reviewModel.reviews.isEmpty ? reviewModel.reviewLoadState.failureMessage : nil, isStatus: true) {
                    Task { await workspaceActions.reload() }
                }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle("Reviews")
        .toolbar {
            if toolbarOwnership.contains(.filter) {
                ToolbarItem(id: "review.project", placement: .navigation) {
                    ProjectFilterMenu(
                        projects: projects,
                        selectedProjectId: filters.projectId,
                        unscopedTitle: String(localized: "All Projects"),
                        unscopedSystemImage: nil,
                        isLoading: false,
                        help: String(localized: "Filter Reviews by Project"),
                        onCreate: nil,
                        onSelect: { filters.projectId = $0 }
                    )
                }
                ToolbarItem(id: "review.filter", placement: .navigation) {
                    ReviewStatusFilterControl(
                        reviews: reviewModel.reviews,
                        selection: $filters.status
                    )
                }
                ToolbarItem(id: "review.author", placement: .navigation) {
                    ToolbarFilterMenu(selectionTitle: authorFilterTitle) {
                        Picker("Author", selection: $filters.authorId) {
                            Text("All Authors").tag(String?.none)
                            ForEach(authors, id: \.userId) { author in
                                Text(authorName(author)).tag(Optional(author.userId))
                            }
                        }
                        .pickerStyle(.inline)
                    }
                    .toolbarHelp(String(localized: "Filter Reviews by Author"))
                    .accessibilityLabel("Author Filter")
                    .accessibilityValue(authorFilterTitle)
                }
            }
        }
    }

    private var authors: [UserReference] {
        Dictionary(
            reviewModel.reviews.map { ($0.author.userId, $0.author) },
            uniquingKeysWith: { first, _ in first }
        )
        .values
        .sorted {
            authorName($0).localizedStandardCompare(authorName($1)) == .orderedAscending
        }
    }

    private var projects: [ProjectState] {
        let reviewProjectIds = Set(reviewModel.reviews.map(\.projectId))
        return workspaceContext.projects.filter { reviewProjectIds.contains($0.id) }
    }

    private var authorFilterTitle: String {
        guard let author = authors.first(where: { $0.userId == filters.authorId }) else {
            return String(localized: "All Authors")
        }
        return authorName(author)
    }

    private func authorName(_ author: UserReference) -> String {
        author.displayName ?? author.email
    }

    private func projectName(for review: ReviewRecord) -> String? {
        workspaceContext.projects.first { $0.id == review.projectId }?.name
    }

    private var trimmedSearchQuery: String {
        searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

enum ReviewListContentState: Equatable {
    case loading
    case failed
    case empty
    case filteredEmpty
    case content

    static func resolve(
        loadState: WorkspaceCollectionLoadState,
        totalCount: Int,
        visibleCount: Int
    ) -> ReviewListContentState {
        if visibleCount > 0 { return .content }
        switch loadState {
        case .loading: return .loading
        case .failed: return .failed
        case .loaded:
            return totalCount == 0 ? .empty : .filteredEmpty
        }
    }
}


struct ReviewQueueStatePresentation: Equatable {
    enum Tone: Equatable {
        case neutral
        case positive
        case done
        case warning
        case negative

        var color: Color {
            switch self {
            case .neutral: .secondary
            case .positive: .green
            case .done: .purple
            case .warning: .orange
            case .negative: .red
            }
        }
    }

    let title: String
    let symbolName: String
    let tone: Tone
    let isQueueSignal: Bool

    static func resolve(
        review: ReviewRecord,
        isAuthor: Bool,
        canMerge: Bool
    ) -> ReviewQueueStatePresentation {
        if review.status == "merged" {
            return .init(
                title: String(localized: "Merged"),
                symbolName: "arrow.triangle.merge",
                tone: .done,
                isQueueSignal: false
            )
        }
        if let reconciliation = ReviewReconciliationState.resolve(
            freshness: review.freshness, reconciliation: review.reconciliation, autoRebased: review.autoRebased
        ) {
            return .init(title: reconciliation.title,
                         symbolName: reconciliation == .conflict ? "exclamationmark.triangle" : "checkmark.circle",
                         tone: reconciliation == .conflict ? .negative : .neutral,
                         isQueueSignal: true)
        }

        switch review.status {
        case "open":
            return .init(
                title: String(localized: "Needs Review"),
                symbolName: "clock",
                tone: .neutral,
                isQueueSignal: false
            )
        case "approved"
            where canMerge && review.approvedResultHash?.isEmpty == false:
            return .init(
                title: String(localized: "Ready to Merge"),
                symbolName: "arrow.triangle.merge",
                tone: .positive,
                isQueueSignal: true
            )
        case "approved":
            return .init(
                title: String(localized: "Approved"),
                symbolName: "checkmark.circle",
                tone: .positive,
                isQueueSignal: false
            )
        case "rejected" where isAuthor:
            return .init(
                title: String(localized: "Resubmit"),
                symbolName: "arrow.clockwise.circle",
                tone: .negative,
                isQueueSignal: true
            )
        case "rejected":
            return .init(
                title: String(localized: "Awaiting Author"),
                symbolName: "clock",
                tone: .neutral,
                isQueueSignal: true
            )
        default:
            return .init(
                title: review.status.capitalized,
                symbolName: "circle",
                tone: .neutral,
                isQueueSignal: false
            )
        }
    }
}

struct ReviewRow: View {
    let review: ReviewRecord
    let projectName: String?
    let state: ReviewQueueStatePresentation
    var errorMessage: String? = nil

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            ReviewSymbolImage(systemName: lifecycleSymbolName)
                .foregroundStyle(lifecycleColor)
                .padding(.top, 2)
                .help(lifecycleTitle)

            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 16) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(review.title)
                            .font(.body.weight(.medium))
                            .lineLimit(2)
                            .help(review.title)

                        if errorMessage != nil {
                            InlineStatusBadge(text: String(localized: "Retry Needed"), color: Color(nsColor: .systemRed))
                        } else if state.isQueueSignal {
                            InlineStatusBadge(text: state.title,
                                              color: ReviewReconciliationState.resolve(
                                                  freshness: review.freshness, reconciliation: review.reconciliation,
                                                  autoRebased: review.autoRebased)?.badgeColor)
                        }
                    }
                    .layoutPriority(1)

                    Spacer(minLength: 0)

                    UserIdentityLabel(account: review.author, displayName: author, size: .small)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: 180, alignment: .trailing)
                        .help("Submitted by \(author)")
                }

                HStack(spacing: 16) {
                    if let projectName, !projectName.isEmpty {
                        Text(projectName)
                    }

                    Spacer(minLength: 0)

                    if let updatedAt = TimestampFormatting.date(from: review.updatedAt) {
                        Text(updatedAt, format: .dateTime.year().month(.twoDigits).day(.twoDigits).hour().minute())
                            .help("Last Review record update")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            }
        }
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibilityText)
    }

    private var author: String {
        review.author.displayName ?? review.author.email
    }

    private var lifecycleSymbolName: String {
        switch review.status {
        case "merged": "arrow.triangle.merge"
        case "approved": "checkmark.circle"
        default: "checkmark.bubble"
        }
    }

    private var lifecycleColor: Color {
        switch review.status {
        case "merged": .purple
        case "rejected": .red
        default: .green
        }
    }

    private var lifecycleTitle: String {
        switch review.status {
        case "merged": String(localized: "Merged Review")
        case "approved": String(localized: "Approved Review")
        case "rejected": String(localized: "Rejected Review")
        default: String(localized: "review.lifecycle.open", defaultValue: "Open Review", comment: "Accessibility description of a review awaiting a decision, not the Open Review button.")
        }
    }

    private var accessibilityText: String {
        let project = projectName.map { ", \($0)" } ?? ""
        let queueState = errorMessage != nil ? String(localized: ", Retry Needed")
            : (state.isQueueSignal ? ", \(state.title)" : "")
        let time = TimestampFormatting.absoluteText(review.updatedAt).map { String(localized: ", updated \($0)") } ?? ""
        return String(localized: "\(review.title), \(lifecycleTitle)\(queueState)\(project), Submitted by \(author)\(time)")
    }
}

struct ReviewStatusIndicator: View {
    let status: String

    static func title(for status: String) -> String {
        switch status {
        case "open": String(localized: "review.status.open", defaultValue: "Open", comment: "Review lifecycle status, not the action that opens a file.")
        case "approved": String(localized: "Approved")
        case "rejected": String(localized: "Rejected")
        case "merged": String(localized: "Merged")
        default: status.capitalized
        }
    }

    var iconOnly = false

    @ViewBuilder
    var body: some View {
        if iconOnly {
            label
                .labelStyle(.iconOnly)
        } else {
            label
        }
    }

    private var label: some View {
        Label {
            Text(Self.title(for: status))
                .foregroundStyle(.secondary)
        } icon: {
            ReviewSymbolImage(systemName: symbol)
                .foregroundStyle(color)
        }
        .font(.caption)
        .help("Status: \(Self.title(for: status))")
        .accessibilityLabel("Status: \(Self.title(for: status))")
    }

    private var symbol: String {
        switch status {
        case "open": "checkmark.bubble"
        case "approved": "checkmark.circle"
        case "rejected": "xmark.circle"
        case "merged": "arrow.triangle.merge"
        default: "circle"
        }
    }

    private var color: Color {
        switch status {
        case "open": .green
        case "approved": .green
        case "rejected": .red
        case "merged": .purple
        default: .secondary
        }
    }
}
