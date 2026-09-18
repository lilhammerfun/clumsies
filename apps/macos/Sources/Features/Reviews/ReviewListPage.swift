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
        .help("Filter Reviews: \(label(for: selection))")
        .accessibilityLabel("Filter Reviews")
        .accessibilityValue(label(for: selection))
        .accessibilityIdentifier("review-toolbar-filter")
    }

    private func label(for filter: ReviewStatusFilter) -> String {
        "\(filter.title) (\(filter.count(in: reviews)))"
    }
}

struct ReviewListPage: View {
    let store: WorkspaceCoordinator
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var reviewModel: ReviewsModel
    let reviews: [ReviewRecord]
    let searchQuery: String
    @Binding var filters: ReviewListFilters
    let toolbarOwnership: ReviewToolbarOwnership
    let onClearFilters: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            if !reviewModel.reviews.isEmpty {
                filterBar
            }

            Group {
                switch ReviewListContentState.resolve(
                    loadState: reviewModel.reviewLoadState,
                    totalCount: reviewModel.reviews.count,
                    visibleCount: reviews.count
                ) {
                case .loading:
                    ContentLoadingView(title: "Loading Reviews…")
                case .failed:
                    ContentUnavailableView {
                        Label("Reviews Unavailable", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(reviewModel.reviewLoadState.failureMessage ?? "Reviews could not be loaded.")
                    } actions: {
                        Button("Try Again") { Task { await store.reload() } }
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
                                canMerge: workspaceContext.canMergeReviews
                            )
                            NavigationLink(value: route) {
                                ReviewRow(
                                    review: review,
                                    projectName: projectName(for: review),
                                    state: state
                                )
                            }
                            .listRowSeparator(.visible)
                            .accessibilityIdentifier("review-row-\(review.id)")
                        }
                    }
                    .listStyle(.inset)
                    .safeAreaInset(edge: .bottom) {
                        ReviewCollectionStatusBanner(store: store)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle("Reviews")
        .toolbar {
            if toolbarOwnership.contains(.filter) {
                ToolbarItem(id: "review.filter", placement: .navigation) {
                    ReviewStatusFilterControl(
                        reviews: reviewModel.reviews,
                        selection: $filters.status
                    )
                }
            }
        }
    }

    private var filterBar: some View {
        VStack(spacing: 0) {
            HStack(spacing: 14) {
                Spacer()

                Menu(authorFilterTitle) {
                    Button {
                        filters.authorId = nil
                    } label: {
                        filterOption("All Authors", isSelected: filters.authorId == nil)
                    }

                    if authors.isEmpty {
                        Button("No Authors") {}
                            .disabled(true)
                    } else {
                        Divider()
                        ForEach(authors, id: \.userId) { author in
                            Button {
                                filters.authorId = author.userId
                            } label: {
                                filterOption(
                                    authorName(author),
                                    isSelected: filters.authorId == author.userId
                                )
                            }
                        }
                    }
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
                .help("Filter Reviews by Author")
                .accessibilityLabel("Author Filter")
                .accessibilityValue(authorFilterTitle)

                Menu(projectFilterTitle) {
                    Button {
                        filters.projectId = nil
                    } label: {
                        filterOption("All Projects", isSelected: filters.projectId == nil)
                    }

                    if projects.isEmpty {
                        Button("No Projects") {}
                            .disabled(true)
                    } else {
                        Divider()
                        ForEach(projects) { project in
                            Button {
                                filters.projectId = project.id
                            } label: {
                                filterOption(
                                    project.name,
                                    isSelected: filters.projectId == project.id
                                )
                            }
                        }
                    }
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
                .help("Filter Reviews by Project")
                .accessibilityLabel("Project Filter")
                .accessibilityValue(projectFilterTitle)
            }
            .controlSize(.small)
            .padding(.horizontal, 12)
            .padding(.vertical, 7)

            Divider()
        }
        .background(.bar)
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
            return "Author"
        }
        return "Author: \(authorName(author))"
    }

    private var projectFilterTitle: String {
        guard let project = projects.first(where: { $0.id == filters.projectId }) else {
            return "Projects"
        }
        return "Project: \(project.name)"
    }

    private func authorName(_ author: UserReference) -> String {
        author.displayName ?? author.email
    }

    @ViewBuilder
    private func filterOption(_ title: String, isSelected: Bool) -> some View {
        if isSelected {
            Label(title, systemImage: "checkmark")
        } else {
            Text(title)
        }
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

private struct ReviewCollectionStatusBanner: View {
    let store: WorkspaceCoordinator
    @EnvironmentObject private var reviewModel: ReviewsModel

    @ViewBuilder
    var body: some View {
        switch reviewModel.reviewLoadState {
        case .loading:
            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                Text("Refreshing Reviews…")
                    .font(.caption)
            }
            .padding(8)
            .frame(maxWidth: .infinity)
            .background(.bar)
        case .failed:
            Button("Review refresh failed — Try Again") {
                Task { await store.reload() }
            }
            .buttonStyle(.plain)
            .font(.caption)
            .padding(8)
            .frame(maxWidth: .infinity)
            .background(.bar)
        case .loaded:
            EmptyView()
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
                title: "Merged",
                symbolName: "arrow.triangle.merge",
                tone: .done,
                isQueueSignal: false
            )
        }
        if review.freshness == .behind, review.reconciliation == .conflicts {
            return .init(
                title: "Conflicts",
                symbolName: "exclamationmark.triangle",
                tone: .warning,
                isQueueSignal: true
            )
        }
        if review.freshness == .behind {
            return .init(
                title: isAuthor ? "Update Required" : "Out of Date",
                symbolName: "arrow.trianglehead.2.clockwise.rotate.90",
                tone: .warning,
                isQueueSignal: true
            )
        }

        switch review.status {
        case "open":
            return .init(
                title: "Needs Review",
                symbolName: "clock",
                tone: .neutral,
                isQueueSignal: false
            )
        case "approved"
            where canMerge && review.approvedResultHash?.isEmpty == false:
            return .init(
                title: "Ready to Merge",
                symbolName: "arrow.triangle.merge",
                tone: .positive,
                isQueueSignal: true
            )
        case "approved":
            return .init(
                title: "Approved",
                symbolName: "checkmark.circle",
                tone: .positive,
                isQueueSignal: false
            )
        case "rejected" where isAuthor:
            return .init(
                title: "Resubmit",
                symbolName: "arrow.clockwise.circle",
                tone: .negative,
                isQueueSignal: true
            )
        case "rejected":
            return .init(
                title: "Awaiting Author",
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

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .top, spacing: 8) {
                ReviewSymbolImage(systemName: lifecycleSymbolName)
                    .foregroundStyle(lifecycleColor)
                    .padding(.top, 2)
                    .help(lifecycleTitle)

                Text(review.title)
                    .font(.body.weight(.medium))
                    .lineLimit(2)
                    .layoutPriority(1)
                    .help(review.title)

                Spacer(minLength: 8)

                if state.isQueueSignal {
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 4) {
                            ReviewSymbolImage(systemName: state.symbolName)
                            Text(state.title)
                        }
                            .lineLimit(1)
                            .fixedSize(horizontal: true, vertical: false)

                        ReviewSymbolImage(systemName: state.symbolName)
                    }
                    .font(.caption)
                    .foregroundStyle(state.tone.color)
                    .help(state.title)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(state.title)
                }
            }

            HStack(alignment: .firstTextBaseline, spacing: 8) {
                metadata
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .help(metadataHelp)

                Spacer(minLength: 8)

                if let updatedAt = TimestampFormatting.date(from: review.updatedAt) {
                    Text(updatedAt, style: .relative)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .fixedSize(horizontal: true, vertical: false)
                        .help(metadataHelp)
                }
            }
        }
        .padding(.vertical, 7)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .alignmentGuide(.listRowSeparatorLeading) { _ in 0 }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibilityText)
    }

    private var author: String {
        review.author.displayName ?? review.author.email
    }

    private var context: String {
        guard let projectName, !projectName.isEmpty else { return author }
        return "\(projectName) · \(author)"
    }

    private var metadata: Text {
        var text = Text("Submitted by \(author)")
        if let projectName, !projectName.isEmpty {
            text = text + Text(" for \(projectName)")
        }
        return text
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
        case "merged": "Merged Review"
        case "approved": "Approved Review"
        case "rejected": "Rejected Review"
        default: "Open Review"
        }
    }

    private var metadataHelp: String {
        TimestampFormatting.absoluteText(review.updatedAt)
            .map { "Last Review record update: \($0)" }
            ?? "Review submission"
    }

    private var accessibilityText: String {
        let relative = TimestampFormatting.relativeText(review.updatedAt, relativeTo: .now)
        let time = relative.map { ", updated \($0)" } ?? ""
        let queueState = state.isQueueSignal ? ", \(state.title)" : ""
        return "\(review.title), \(lifecycleTitle)\(queueState), \(context)\(time)"
    }
}

struct ReviewStatusIndicator: View {
    let status: String
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
            Text(status.capitalized)
                .foregroundStyle(.secondary)
        } icon: {
            ReviewSymbolImage(systemName: symbol)
                .foregroundStyle(color)
        }
        .font(.caption)
        .help("Status: \(status.capitalized)")
        .accessibilityLabel("Status: \(status.capitalized)")
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
