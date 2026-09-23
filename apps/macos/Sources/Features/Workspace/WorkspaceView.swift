import AppKit
import SwiftUI

extension ToolbarItemPlacement {
    /// Pins toolbar content to the trailing edge on both macOS 14 (where
    /// `.primaryAction` is trailing) and macOS 26 Liquid Glass (where
    /// primary-action items render centered and the trailing edge is reached
    /// with `.automatic` items pushed by a `ToolbarSpacer(.flexible)`).
    static var trailingPinned: ToolbarItemPlacement {
        if #available(macOS 26.0, *) { return .automatic }
        return .primaryAction
    }
}

enum WorkspaceColumnLayout: Equatable {
    case sidebarDetail
    case sidebarContentDetail

    init(section: WorkspaceSection) {
        self = [.reviews, .dashboard, .inbox].contains(section)
            ? .sidebarDetail
            : .sidebarContentDetail
    }
}

struct WorkspaceView: View {
    @EnvironmentObject private var bundleStore: BundleStore
    let store: WorkspaceCoordinator
    @EnvironmentObject private var bundleModel: BundlesModel
    @EnvironmentObject private var memoryCatalog: MemoryCatalog
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var draftStore: DraftStore
    @EnvironmentObject private var workspaceFeedback: WorkspaceFeedback
    @EnvironmentObject private var memoryModel: MemoryModel
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation
    @EnvironmentObject private var reconciler: DraftReconciliationService
    @EnvironmentObject private var reviewModel: ReviewsModel
    @EnvironmentObject private var documentSessions: DocumentSessions
    @EnvironmentObject private var inbox: InboxStore
    let onSignOut: () -> Void
    let onOpenSettings: () -> Void
    let loadsReviewDetail: Bool
    @StateObject private var activityModel: ActivityModel
    @State private var splitVisibility: NavigationSplitViewVisibility = .all
    @State private var dashboardSplitVisibility: NavigationSplitViewVisibility = .all
    @State private var reviewSplitVisibility: NavigationSplitViewVisibility = .all
    @State private var activitySplitVisibility: NavigationSplitViewVisibility = .all
    @State private var inboxSplitVisibility: NavigationSplitViewVisibility = .all
    @State private var showsBundleResourcePicker = false
    @State private var confirmsBundleDeletion = false
    @State private var reviewNavigationPath: [ReviewRoute] = []
    @State private var workspaceSearchFocusRequest = 0
    @State private var reviewSearchQuery = ""
    @State private var reviewSearchFocusRequest = 0
    @State private var reviewFilters = ReviewListFilters()
    @State private var pendingReviewToolbarAction: ReviewMenuAction?
    private struct ProjectReviewRequest: Identifiable {
        let id = UUID()
        let drafts: [LocalDraft]
        let title: String
    }

    @State private var pendingProjectReview: ProjectReviewRequest?

    init(
        store: WorkspaceCoordinator,
        onSignOut: @escaping () -> Void,
        onOpenSettings: @escaping () -> Void,
        loadsReviewDetail: Bool = true
    ) {
        self.store = store
        self.onSignOut = onSignOut
        self.onOpenSettings = onOpenSettings
        self.loadsReviewDetail = loadsReviewDetail
        _activityModel = StateObject(wrappedValue: ActivityModel(daemon: store.context.daemon))
    }

    private var showsDocumentTabs: Bool {
        workspaceNavigation.selectedSection == .memory
            && !workspaceNavigation.visibleTabs.isEmpty
            && !workspaceNavigation.showsProjectSettings
    }

    private var showsMemoryContentToolbar: Bool {
        workspaceNavigation.selectedSection == .memory && !workspaceNavigation.showsProjectSettings
    }

    private func deferSidebarExpansionUpdate(_ expanded: Bool) {
        guard workspaceNavigation.sidebarExpanded != expanded else { return }
        DispatchQueue.main.async {
            if workspaceNavigation.sidebarExpanded != expanded {
                workspaceNavigation.sidebarExpanded = expanded
            }
        }
    }

    var body: some View {
        Group {
            switch workspaceNavigation.selectedSection {
            case .dashboard:
                dashboardWorkspace
            case .inbox:
                inboxWorkspace
            case .reviews:
                reviewsWorkspace
            case .sessions:
                activityWorkspace
            default:
                regularWorkspace
            }
        }
        .feedbackHost(error: workspaceFeedback.errorMessage, dismiss: workspaceFeedback.dismissErrorMessage)
        .sheet(isPresented: $workspaceNavigation.showsLocalProjectRecovery) {
            LocalProjectRecoveryView(store: inbox, retry: { await store.refresh.retrySync(allProjects: true, reportFailure: false) })
        }
        .sheet(isPresented: $workspaceNavigation.showsProjectCreation) {
            ProjectCreationSheet(model: ProjectCreationModel(projects: store.projects))
        }
        .onChange(of: workspaceNavigation.selectedSection) { _, _ in
            DispatchQueue.main.async {
                workspaceNavigation.searchQuery = ""
                if workspaceNavigation.selectedSection != .memory {
                    workspaceNavigation.showsProjectSettings = false
                }
            }
        }
        .task {
            await store.runRefreshLoop()
        }
    }

    private var dashboardWorkspace: some View {
        NavigationSplitView(columnVisibility: $dashboardSplitVisibility) {
            GlobalSidebar(store: store, onSignOut: onSignOut, onOpenSettings: onOpenSettings)
                .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 280)
        } detail: {
            DashboardPage(context: workspaceContext) { id in
                guard let item = workspaceNavigation.memoryItems.first(where: { $0.id == id }) else { return }
                workspaceNavigation.selectedSection = .memory
                workspaceNavigation.open(item)
            }
            .toolbar {
                ToolbarItem(placement: .navigation) { MemoryProjectFilter(store: store) }
            }
        }
        .onAppear {
            dashboardSplitVisibility = workspaceNavigation.sidebarExpanded ? .all : .detailOnly
        }
        .onChange(of: dashboardSplitVisibility) { _, visibility in
            deferSidebarExpansionUpdate(visibility != .detailOnly)
        }
        .onChange(of: workspaceNavigation.sidebarExpanded) { _, expanded in
            dashboardSplitVisibility = expanded ? .all : .detailOnly
        }
    }

    private var inboxWorkspace: some View {
        NavigationSplitView(columnVisibility: $inboxSplitVisibility) {
            GlobalSidebar(store: store, onSignOut: onSignOut, onOpenSettings: onOpenSettings)
                .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 280)
        } detail: {
            InboxView(store: inbox,
                searchFocusToken: workspaceNavigation.workspaceSearchFocusToken,
                open: { try await store.openInboxDestination($0) })
                .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
        }
        .onAppear { inboxSplitVisibility = workspaceNavigation.sidebarExpanded ? .all : .detailOnly }
        .onChange(of: inboxSplitVisibility) { _, value in deferSidebarExpansionUpdate(value != .detailOnly) }
        .onChange(of: workspaceNavigation.sidebarExpanded) { _, expanded in
            inboxSplitVisibility = expanded ? .all : .detailOnly
        }
    }

    private var regularWorkspace: some View {
        NavigationSplitView(columnVisibility: $splitVisibility) {
                GlobalSidebar(
                    store: store,
                    onSignOut: onSignOut,
                    onOpenSettings: onOpenSettings
                )
                .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 280)
            } content: {
                navigator
                    .navigationSplitViewColumnWidth(min: 288, ideal: 300, max: 380)
                    .toolbar {
                        navigationToolbarContent
                    }
            } detail: {
                VStack(spacing: 0) {
                    detail
                        .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
                }
                .toolbar {
                    if showsMemoryContentToolbar {
                        ToolbarItemGroup {
                            Button {
                                workspaceNavigation.goBack()
                            } label: {
                                Image(systemName: "chevron.left")
                            }
                            .disabled(!workspaceNavigation.canGoBack)
                            .toolbarHelp(String(localized: "Go Back"))
                            .accessibilityLabel("Go Back")

                            Button {
                                workspaceNavigation.goForward()
                            } label: {
                                Image(systemName: "chevron.right")
                            }
                            .disabled(!workspaceNavigation.canGoForward)
                            .toolbarHelp(String(localized: "Go Forward"))
                            .accessibilityLabel("Go Forward")
                        }
                    }

                    if #available(macOS 26.0, *) {
                        ToolbarSpacer(.flexible)
                    }

                    ToolbarItemGroup(placement: .trailingPinned) {
                        if workspaceNavigation.selectedSection == .bundles, bundleModel.selectedBundle != nil {
                            Button {
                                showsBundleResourcePicker = true
                            } label: {
                                Image(systemName: "plus")
                            }
                            .toolbarHelp(String(localized: "Add Memory"))
                            .accessibilityLabel("Add Memory")
                        }

                        if showsDocumentTabs, let item = workspaceNavigation.currentItem {
                            Picker("Document View", selection: documentMode) {
                                ForEach(availableDocumentModes, id: \.self) { mode in
                                    Text(mode.title).tag(mode)
                                }
                            }
                            .pickerStyle(.segmented)
                            .disabled(
                                availableDocumentModes.count < 2
                                    || documentSessions.isSynchronizingDocument(item.id)
                            )
                            .toolbarHelp(String(localized: "Document View"))
                            .accessibilityLabel("Document View")

                        }

                        if workspaceNavigation.selectedSection == .bundles, bundleModel.selectedBundle != nil {
                            Menu {
                                Button("Delete Bundle", role: .destructive) {
                                    confirmsBundleDeletion = true
                                }
                            } label: {
                                Image(systemName: "ellipsis")
                            }
                            .menuIndicator(.hidden)
                            .toolbarHelp(String(localized: "Bundle Actions"))
                            .accessibilityLabel("Bundle Actions")
                        }

                        if showsMemoryContentToolbar {
                            Menu {
                                Button(workspaceContext.activeProjectId == nil
                                    ? "Export Organization Memory as ZIP…"
                                    : "Export Project Memory as ZIP…") {
                                    memoryModel.exportMemory()
                                }
                                .disabled(!memoryModel.canExportMemory(memoryModel.visibleMemoryItems))
                                if let item = workspaceNavigation.currentItem {
                                    Button("Export File as ZIP…") {
                                        memoryModel.exportMemory([item], name: item.document.title)
                                    }
                                    .disabled(!memoryModel.canExportMemory([item]))
                                    Divider()
                                }
                                if let item = workspaceNavigation.currentItem, hasDocumentActions(item) {
                                    if let draft = item.draft, draft.status == .submitted {
                                        Button("View Review") {
                                            Task { await reviewModel.openReview(for: draft) }
                                        }
                                        Divider()
                                    }
                                    if canRequestDocumentReview(item),
                                       let draft = item.draft,
                                       let sessionKey = documentSessions.documentSessionKey(for: item) {
                                        Button("Request Review…") {
                                            workspaceNavigation.pendingDocumentCommand = .requestReview(
                                                sessionKey: sessionKey,
                                                draft: draft
                                            )
                                        }
                                        .disabled(documentSessions.isSynchronizingDocument(item.id))
                                        Divider()
                                    }
                                    if canDiscardDocumentDraft(item),
                                       let draft = item.draft,
                                       let sessionKey = documentSessions.documentSessionKey(for: item) {
                                        Button("Discard Draft") {
                                            workspaceNavigation.pendingDocumentCommand = .discardDraft(
                                                sessionKey: sessionKey,
                                                draft: draft
                                            )
                                        }
                                        .disabled(documentSessions.isSynchronizingDocument(item.id))
                                    }
                                    if canProposeMemoryDeletion(item),
                                       let sessionKey = documentSessions.documentSessionKey(for: item) {
                                        Button(
                                            "Delete…",
                                            role: .destructive
                                        ) {
                                            workspaceNavigation.pendingDocumentCommand = .moveToTrash(
                                                sessionKey: sessionKey
                                            )
                                        }
                                        .disabled(documentSessions.isSynchronizingDocument(item.id))
                                    }
                                    Divider()
                                }

                                Button("Request Review for All Project Changes…") {
                                    pendingProjectReview = ProjectReviewRequest(
                                        drafts: activeProjectReviewDrafts,
                                        title: String(localized: "Update \(workspaceContext.activeProject?.name ?? "project") memory")
                                    )
                                }
                                .disabled(activeProjectReviewDrafts.isEmpty)
                            } label: {
                                Image(systemName: "ellipsis")
                            }
                            .menuIndicator(.hidden)
                            .toolbarHelp(String(localized: "Memory Actions"))
                            .accessibilityLabel("Memory Actions")
                        }
                    }

                    if #available(macOS 26.0, *) {
                        ToolbarSpacer(.fixed, placement: .automatic)
                    }

                    ToolbarItem(id: "workspace.search", placement: .trailingPinned) {
                        ClassicSearchField(
                            text: $workspaceNavigation.searchQuery,
                            prompt: workspaceSearchPrompt,
                            accessibilityIdentifier: "workspace-toolbar-search",
                            accessibilityHelp: String(localized: "Search across the current workspace"),
                            focusToken: workspaceSearchFocusRequest
                        )
                    }
                }
            }
        .onAppear {
            let target: NavigationSplitViewVisibility = workspaceNavigation.sidebarExpanded ? .all : .doubleColumn
            if splitVisibility != target {
                splitVisibility = target
            }
        }
        .onChange(of: workspaceNavigation.workspaceSearchFocusToken) { _, _ in
            workspaceSearchFocusRequest += 1
        }
        .onChange(of: workspaceNavigation.searchQuery) { _, query in
            guard workspaceNavigation.selectedSection == .memory,
                  !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
            Task { await store.prepareWorkspaceIndex(includeContent: true) }
        }
        .onChange(of: splitVisibility) { _, visibility in
            if visibility == .all {
                deferSidebarExpansionUpdate(true)
            } else if visibility == .doubleColumn || visibility == .detailOnly {
                deferSidebarExpansionUpdate(false)
            }
        }
        .onChange(of: workspaceNavigation.sidebarExpanded) { _, expanded in
            let target: NavigationSplitViewVisibility = expanded ? .all : .doubleColumn
            if splitVisibility != target {
                splitVisibility = target
            }
        }
        .sheet(item: $pendingProjectReview) { request in
            ReviewRequestSheet(
                initialTitle: request.title,
                drafts: request.drafts,
                loadCandidates: {
                    try await reconciler.reconciliationCandidates(for: request.drafts)
                }
            ) { title, description, reconciliations, contributions in
                try await reviewModel.requestReview(
                    for: request.drafts,
                    title: title,
                    description: description,
                    reconciliations: reconciliations,
                    contributions: contributions
                )
            }
        }
    }

    private var reviewsWorkspace: some View {
        NavigationSplitView(columnVisibility: $reviewSplitVisibility) {
            GlobalSidebar(
                store: store,
                onSignOut: onSignOut,
                onOpenSettings: onOpenSettings
            )
            .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 280)
        } detail: {
            NavigationStack(path: $reviewNavigationPath) {
                ReviewListPage(reviews: filteredReviews,
                    searchQuery: reviewSearchQuery,
                    filters: $reviewFilters,
                    toolbarOwnership: reviewToolbarOwnership,
                    onClearFilters: {
                        reviewFilters = ReviewListFilters(status: .all)
                        reviewSearchQuery = ""
                    }
                )
                .navigationDestination(for: ReviewRoute.self) { route in
                    ReviewDetailPage(reviewId: route.reviewId,
                        loadsRemoteContent: loadsReviewDetail,
                        model: ReviewDetailModel(reviewId: route.reviewId, context: store.context, feedback: store.feedback, reviews: store.reviews)
                    )
                    .toolbar {
                        if reviewToolbarOwnership.surface == .detail {
                            reviewDetailToolbarContent
                        }
                    }
                }
            }
            .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
            .toolbar {
                if reviewToolbarOwnership.surface == .list {
                    if #available(macOS 26.0, *) {
                        ToolbarSpacer(.flexible, placement: .automatic)
                    }

                    reviewUtilityToolbarContent(hasLeadingActions: false)
                }
            }
        }
        .onAppear {
            let target: NavigationSplitViewVisibility = workspaceNavigation.sidebarExpanded ? .all : .detailOnly
            if reviewSplitVisibility != target {
                reviewSplitVisibility = target
            }

            if let routedReviewId = reviewNavigationPath.last?.reviewId,
               !reviewModel.reviews.contains(where: { $0.id == routedReviewId }) {
                reviewNavigationPath.removeAll()
            }
            if let reviewId = reviewModel.selectedReviewId,
               reviewNavigationPath.last?.reviewId != reviewId,
               reviewModel.reviews.contains(where: { $0.id == reviewId }) {
                reviewNavigationPath = [ReviewRoute(reviewId: reviewId)]
            }
        }
        .onChange(of: reviewSplitVisibility) { _, visibility in
            let expanded = visibility != .detailOnly
            deferSidebarExpansionUpdate(expanded)
        }
        .onChange(of: workspaceNavigation.sidebarExpanded) { _, expanded in
            let target: NavigationSplitViewVisibility = expanded ? .all : .detailOnly
            if reviewSplitVisibility != target {
                reviewSplitVisibility = target
            }
        }
        .onChange(of: reviewNavigationPath) { _, path in
            let reviewId = path.last?.reviewId
            DispatchQueue.main.async {
                if reviewModel.selectedReviewId != reviewId {
                    reviewModel.selectedReviewId = reviewId
                }
                if reviewId == nil {
                    reviewModel.reviewDecisionReadiness = nil
                }
            }
            if reviewId == nil {
                pendingReviewToolbarAction = nil
            }
        }
        .onChange(of: workspaceNavigation.reviewSearchFocusToken) { _, _ in
            reviewNavigationPath.removeAll()
            DispatchQueue.main.async {
                reviewSearchFocusRequest += 1
            }
        }
        .onChange(of: reviewModel.selectedReviewId) { _, reviewId in
            guard workspaceNavigation.selectedSection == .reviews else { return }
            guard let reviewId else {
                if !reviewNavigationPath.isEmpty {
                    reviewNavigationPath.removeAll()
                }
                return
            }
            guard reviewNavigationPath.last?.reviewId != reviewId else { return }
            guard reviewModel.reviews.contains(where: { $0.id == reviewId }) else { return }
            reviewNavigationPath = [ReviewRoute(reviewId: reviewId)]
        }
    }

    private var workspaceSearchPrompt: String {
        switch workspaceNavigation.selectedSection {
        case .dashboard: String(localized: "Search Dashboard")
        case .memory: String(localized: "Search Memory")
        case .bundles: String(localized: "Search Bundles")
        case .reviews: String(localized: "Search Reviews")
        case .sessions: String(localized: "Search Activity")
        case .inbox: String(localized: "Search Inbox")
        }
    }

    private var filteredReviews: [ReviewRecord] {
        let byFilters = reviewModel.reviews.filter(reviewFilters.matches)
        let needle = reviewSearchQuery.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        guard !needle.isEmpty else { return byFilters }
        return byFilters.filter {
            "\($0.title) \($0.description) \($0.author.email) \($0.status)"
                .localizedLowercase.contains(needle)
        }
    }

    private var reviewToolbarOwnership: ReviewToolbarOwnership {
        let review = selectedReviewForToolbar
        return .resolve(
            surface: reviewNavigationPath.isEmpty ? .list : .detail,
            review: review,
            canDecideReviews: review.map(workspaceContext.canDecideReview) ?? false,
            canMergeReviews: review.map(workspaceContext.canMergeReview) ?? false,
            isAuthor: review.map(workspaceContext.isReviewAuthor) ?? false
        )
    }

    private var selectedReviewForToolbar: ReviewRecord? {
        guard let reviewId = reviewModel.selectedReviewId else { return nil }
        return reviewModel.reviews.first { $0.id == reviewId }
    }

    @ToolbarContentBuilder
    private var reviewDetailToolbarContent: some ToolbarContent {
        if #available(macOS 26.0, *) {
            ToolbarSpacer(.flexible, placement: .automatic)
        }

        if let review = selectedReviewForToolbar {
            if reviewToolbarOwnership.contains(.decision(.reject)) {
                ToolbarItem(id: "review.reject", placement: .automatic) {
                    Button {
                        performReviewToolbarAction(.reject)
                    } label: {
                        reviewToolbarActionLabel(
                            systemImage: "xmark",
                            isPending: pendingReviewToolbarAction == .reject
                        )
                    }
                    .disabled(
                        pendingReviewToolbarAction != nil
                            || !reviewModel.canPerformReviewMenuAction(.reject)
                    )
                    .toolbarHelp(review.freshness == .behind
                        ? String(localized: "Reject Review — update to the latest remote version before deciding")
                        : String(localized: "Reject Review"))
                    .accessibilityLabel("Reject Review")
                    .accessibilityIdentifier("review-toolbar-reject")
                }
            }

            if reviewToolbarOwnership.contains(.decision(.approve)) {
                ToolbarItem(id: "review.approve", placement: .automatic) {
                    Button {
                        performReviewToolbarAction(.approve)
                    } label: {
                        reviewToolbarActionLabel(
                            systemImage: "checkmark",
                            isPending: pendingReviewToolbarAction == .approve
                        )
                    }
                    .disabled(
                        pendingReviewToolbarAction != nil
                            || !reviewModel.canPerformReviewMenuAction(.approve)
                    )
                    .toolbarHelp(review.freshness == .behind
                        ? String(localized: "Approve and Merge Review — available after conflicts are resolved and remote changes are saved")
                        : String(localized: "Approve and Merge Review"))
                    .accessibilityLabel("Approve and Merge Review")
                    .accessibilityIdentifier("review-toolbar-approve")
                }
            }
            if reviewModel.canSaveConflictResolutions(review)
                || reviewToolbarOwnership.contains(.decision(.merge))
                || reviewToolbarOwnership.contains(.decision(.resubmit)) {
                ToolbarItem(id: "review.actions", placement: .automatic) {
                    Menu {
                        if reviewModel.canSaveConflictResolutions(review), let update = reviewModel.updates[review.id] {
                            ReviewUpdateMenuItem(model: update) { detail in
                                reviewModel.endUpdate(review.id, result: detail)
                            }
                        }
                        if reviewToolbarOwnership.contains(.decision(.merge)) {
                            Button("Merge Review") { performReviewToolbarAction(.merge) }
                                .disabled(!reviewModel.canPerformReviewMenuAction(.merge))
                        }
                        if reviewToolbarOwnership.contains(.decision(.resubmit)) {
                            Button("Resubmit Review") { performReviewToolbarAction(.resubmit) }
                                .disabled(!reviewModel.canPerformReviewMenuAction(.resubmit))
                        }
                    } label: {
                        reviewToolbarActionLabel(
                            systemImage: "ellipsis", isPending: pendingReviewToolbarAction != nil)
                    }
                    .menuIndicator(.hidden)
                    .disabled(pendingReviewToolbarAction != nil)
                    .toolbarHelp(String(localized: "Review Actions"))
                    .accessibilityLabel("Review Actions")
                    .accessibilityIdentifier("review-toolbar-actions")
                }
            }
        }

        reviewUtilityToolbarContent(hasLeadingActions: reviewToolbarOwnership.hasDecisionActions
            || reviewModel.selectedReviewId.flatMap { reviewModel.updates[$0] } != nil)
    }

    @ToolbarContentBuilder
    private func reviewUtilityToolbarContent(hasLeadingActions: Bool) -> some ToolbarContent {
        if #available(macOS 26.0, *), hasLeadingActions {
            ToolbarSpacer(.fixed, placement: .automatic)
        }

        if reviewToolbarOwnership.contains(.search) {
            ToolbarItem(id: "review.search", placement: .trailingPinned) {
                ClassicSearchField(
                    text: $reviewSearchQuery,
                    prompt: String(localized: "Search Reviews"),
                    accessibilityIdentifier: "review-toolbar-search",
                    accessibilityHelp: String(localized: "Search Reviews by title, description or author"),
                    focusToken: reviewSearchFocusRequest
                )
            }
        }
    }

    private func performReviewToolbarAction(_ action: ReviewMenuAction) {
        guard pendingReviewToolbarAction == nil else { return }
        pendingReviewToolbarAction = action
        Task {
            defer { pendingReviewToolbarAction = nil }
            await reviewModel.performReviewMenuAction(action)
        }
    }

    private func reviewToolbarActionLabel(systemImage: String, isPending: Bool) -> some View {
        ZStack {
            ReviewSymbolImage(systemName: systemImage)
                .opacity(isPending ? 0 : 1)
            if isPending {
                ProgressView()
                    .controlSize(.small)
            }
        }
        .frame(width: 16, height: 16)
    }

    private var activityWorkspace: some View {
        NavigationSplitView(columnVisibility: $activitySplitVisibility) {
            GlobalSidebar(
                store: store,
                onSignOut: onSignOut,
                onOpenSettings: onOpenSettings
            )
            .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 280)
        } detail: {
            NavigationStack {
                HSplitView {
                    ActivitySessionList(model: activityModel)
                        .frame(minWidth: 260, idealWidth: 300, maxWidth: 380, maxHeight: .infinity)
                    ActivitySessionDetail(model: activityModel)
                        .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
                }
                .navigationDestination(item: Binding(
                    get: { activityModel.retrievalSelection },
                    set: { if $0 == nil { activityModel.closeRetrieval() } }
                )) { selection in
                    ActivityRetrievalDetail(
                        selection: selection,
                        daemon: workspaceContext.daemon
                    )
                }
            }
            .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
            .toolbar {
                if activityModel.retrievalSelection == nil {
                    activityToolbarContent
                }
            }
        }
        .onAppear {
            let target: NavigationSplitViewVisibility = workspaceNavigation.sidebarExpanded ? .all : .detailOnly
            if activitySplitVisibility != target {
                activitySplitVisibility = target
            }
        }
        .task(id: activityProjectContext) {
            activityModel.prepare(
                projectIds: workspaceContext.projects.map(\.id),
                preferredProjectId: workspaceContext.activeProjectId,
                scope: activityPreferenceScope
            )
            if !activityModel.hasLoaded { await activityModel.load() }
        }
        .onChange(of: activitySplitVisibility) { _, visibility in
            deferSidebarExpansionUpdate(visibility != .detailOnly)
        }
        .onChange(of: workspaceNavigation.sidebarExpanded) { _, expanded in
            let target: NavigationSplitViewVisibility = expanded ? .all : .detailOnly
            if activitySplitVisibility != target {
                activitySplitVisibility = target
            }
        }
    }

    private var activityPreferenceScope: String {
        "\(ClumsiesIdentifiers.serverURL.absoluteString)|\(workspaceContext.organization?.orgId ?? "")|\(workspaceContext.account?.userId ?? "")"
    }

    private var activityProjectContext: String {
        activityPreferenceScope + "|" + workspaceContext.projects.map(\.id).joined(separator: "|")
    }

    @ToolbarContentBuilder
    private var activityToolbarContent: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            ActivityProjectFilter(store: store, model: activityModel)
        }

        if #available(macOS 26.0, *) {
            ToolbarSpacer(.flexible, placement: .automatic)
        }

        ToolbarItem(placement: .trailingPinned) {
            Button {
                Task { await activityModel.load() }
            } label: {
                if activityModel.isLoading && !activityModel.sessions.isEmpty {
                    ProgressView().controlSize(.small)
                } else {
                    Image(systemName: "arrow.clockwise")
                }
            }
            .disabled(activityModel.isLoading)
            .toolbarHelp(String(localized: "Refresh Activity"))
            .accessibilityLabel("Refresh Activity")
        }
    }

    @ToolbarContentBuilder
    private var navigationToolbarContent: some ToolbarContent {
        switch workspaceNavigation.selectedSection {
        case .dashboard:
            ToolbarItem { EmptyView() }
        case .memory:
            ToolbarItem(placement: .navigation) {
                MemoryProjectFilter(store: store)
            }

            ToolbarItem {
                Button {
                    workspaceNavigation.showsProjectSettings.toggle()
                } label: {
                    Image(systemName: "gearshape")
                }
                .disabled(workspaceContext.activeProjectId == nil)
                .toolbarHelp(String(localized: "Project Settings"))
                .accessibilityLabel("Project Settings")
            }
        case .bundles:
            ToolbarItem {
                Button {
                    Task { await bundleModel.createBundle() }
                } label: {
                    Image(systemName: "plus")
                }
                .toolbarHelp(String(localized: "New Bundle"))
                .accessibilityLabel("New Bundle")
            }
        case .reviews:
            ToolbarItem {
                EmptyView()
            }
        case .sessions, .inbox:
            ToolbarItem {
                EmptyView()
            }
        }
    }

    @ViewBuilder
    private var navigator: some View {
        switch workspaceNavigation.selectedSection {
        case .dashboard:
            EmptyView()
        case .memory:
            MemoryNavigator()
        case .bundles:
            BundleNavigator()
        case .reviews:
            EmptyView()
        case .sessions, .inbox:
            EmptyView()
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch workspaceNavigation.selectedSection {
        case .dashboard:
            EmptyView()
        case .memory:
            if workspaceContext.projects.isEmpty, !memoryCatalog.resources.contains(where: { $0.scope == .org }) {
                ProjectUnavailableView()
            } else if workspaceNavigation.showsProjectSettings, let projectId = workspaceContext.activeProjectId {
                ProjectSettingsView(projectId: projectId)
            } else {
                MemoryMainPane()
            }
        case .bundles:
            BundleDetail(showsResourcePicker: $showsBundleResourcePicker,
                confirmsDeletion: $confirmsBundleDeletion
            )
        case .reviews:
            EmptyView()
        case .sessions, .inbox:
            EmptyView()
        }
    }

    private var availableDocumentModes: [WorkbenchTabMode] {
        if workspaceNavigation.currentItem?.draft?.documentBaselineAvailable == false {
            return [.diff]
        }
        return workspaceNavigation.currentItem?.supportsMarkdownPreview == true
            ? [.preview, .source, .diff]
            : [.source, .diff]
    }

    private func canRequestDocumentReview(_ item: MemoryListItem) -> Bool {
        guard workspaceContext.activeProjectId != nil,
              let draft = item.draft else {
            return false
        }
        return ReviewsModel.canRequestReview(draft)
    }

    private func canProposeMemoryDeletion(_ item: MemoryListItem) -> Bool {
        draftStore.canEditMemory(item)
            && MemoryFileTreeMenu.canProposeMemoryDeletion(
                item,
                inOrgView: workspaceContext.activeProjectId == nil
            )
    }

    private func canDiscardDocumentDraft(_ item: MemoryListItem) -> Bool {
        workspaceContext.activeProjectId != nil && item.draft != nil
    }

    private func hasDocumentActions(_ item: MemoryListItem) -> Bool {
        canRequestDocumentReview(item)
            || canDiscardDocumentDraft(item)
            || canProposeMemoryDeletion(item)
    }

    private var activeProjectReviewDrafts: [LocalDraft] {
        ReviewsModel.reviewableProjectDrafts(
            draftStore.drafts,
            projectId: workspaceContext.activeProjectId
        )
    }

    private var documentMode: Binding<WorkbenchTabMode> {
        Binding(
            get: {
                let mode = workspaceNavigation.currentTabMode ?? .preview
                return availableDocumentModes.contains(mode) ? mode : .source
            },
            set: { workspaceNavigation.switchDocumentMode($0) }
        )
    }


}

private struct MemoryProjectFilter: View {
    let store: WorkspaceCoordinator
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation

    var body: some View {
        ProjectFilterMenu(
            projects: workspaceContext.projects,
            selectedProjectId: workspaceContext.activeProjectId,
            unscopedTitle: String(localized: "Org"),
            unscopedSystemImage: "building.2",
            isLoading: workspaceContext.isSwitchingMemoryContext,
            help: String(localized: "Filter Memory by Project"),
            onCreate: workspaceContext.canCreateProject ? { workspaceNavigation.presentProjectCreation() } : nil
        ) { projectId in
            if let projectId {
                Task { await store.selectProject(projectId) }
            } else {
                Task { await store.showOrgMemory() }
            }
        }
    }
}

private struct ActivityProjectFilter: View {
    let store: WorkspaceCoordinator
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation
    @ObservedObject var model: ActivityModel

    var body: some View {
        ProjectFilterMenu(
            projects: workspaceContext.projects,
            selectedProjectId: model.selectedProjectId,
            unscopedTitle: String(localized: "All Projects"),
            unscopedSystemImage: nil,
            isLoading: false,
            help: String(localized: "Filter Activity by Project"),
            onCreate: workspaceContext.canCreateProject ? { workspaceNavigation.presentProjectCreation() } : nil
        ) { projectId in
            Task { await model.selectProject(projectId) }
        }
    }
}

private struct GlobalSidebar: View {
    let store: WorkspaceCoordinator
    @EnvironmentObject private var inbox: InboxStore
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation
    @EnvironmentObject private var softwareUpdateController: SoftwareUpdateController
    let onSignOut: () -> Void
    let onOpenSettings: () -> Void

    var body: some View {
        List(selection: selection) {
            Section {
                ForEach(WorkspaceSection.allCases) { section in
                    SidebarDestinationLabel(section: section)
                        .badge(section == .inbox ? inbox.unreadCount : 0)
                        .tag(GlobalSidebarDestination.section(section))
                }
            } header: {
                HStack(spacing: 7) {
                    Image("BrandMark", bundle: .main)
                        .resizable()
                        .scaledToFit()
                        .frame(width: 16, height: 16)
                    Text(workspaceContext.organization?.name ?? "Clumsies Lab")
                        .fontWeight(.semibold)
                        .lineLimit(1)
                }
                .textCase(nil)
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                Divider()
                accountMenu
                    .frame(height: 40)
            }
        }
    }

    private var accountMenu: some View {
        HStack(spacing: 0) {
            NativeAccountMenu(
                account: workspaceContext.account,
                displayName: accountDisplayName,
                onOpenSettings: onOpenSettings,
                onSignOut: onSignOut
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            if softwareUpdateController.hasAvailableUpdate {
                Button("Update", action: softwareUpdateController.checkForUpdates)
                    .buttonStyle(.borderedProminent)
                    .buttonBorderShape(.capsule)
                    .tint(.accentColor)
                    .controlSize(.small)
                    .disabled(!softwareUpdateController.canCheckForUpdates)
                    .help("Check for updates or continue installing an update")
                    .accessibilityIdentifier("softwareUpdateButton")
                    .padding(.trailing, 10)
            }
        }
    }

    private var accountDisplayName: String {
        if let displayName = workspaceContext.account?.displayName?.trimmingCharacters(in: .whitespacesAndNewlines),
           !displayName.isEmpty {
            return displayName
        }
        return workspaceContext.account?.email ?? String(localized: "Account")
    }

    private var selection: Binding<GlobalSidebarDestination?> {
        Binding(
            get: { .section(workspaceNavigation.selectedSection) },
            set: { destination in
                guard let destination else { return }
                if case .section(let section) = destination {
                    DispatchQueue.main.async {
                        workspaceNavigation.selectedSection = section
                        workspaceNavigation.selectedItemId = nil
                    }
                }
            }
        )
    }

}

private struct SidebarDestinationLabel: View {
    let section: WorkspaceSection

    var body: some View {
        HStack(spacing: 8) {
            ReviewSymbolImage(systemName: section.symbol)
                .frame(width: 16)
            Text(section.title)
            Spacer(minLength: 8)
        }
        .contentShape(Rectangle())
    }
}

private enum GlobalSidebarDestination: Hashable {
    case section(WorkspaceSection)
}
