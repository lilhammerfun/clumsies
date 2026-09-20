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

enum SyncToolbarPresentation: Equatable {
    case syncing(changeCount: Int)
    case inReview(changeCount: Int)
    case failed(changeCount: Int, message: String?)
    case unavailable(message: String?)
    case stale

    static func resolve(
        status: DaemonSyncStatus?,
        isAvailable: Bool,
        serverDataSource: String?,
        submittedDraftCount: Int = 0
    ) -> Self? {
        guard isAvailable else { return .unavailable(message: nil) }

        guard let status else {
            return serverDataSource == "stale" ? .stale : .unavailable(message: nil)
        }
        if status.failedOperationCount > 0 || status.draftSync.state == "failed" {
            return .failed(
                changeCount: status.failedOperationCount,
                message: status.draftSync.lastError?.message
            )
        }
        if status.draftSync.state == "degraded" {
            return .unavailable(message: status.draftSync.lastError?.message)
        }
        if status.pendingOperationCount > 0
            || ["queued", "syncing", "retrying"].contains(status.draftSync.state) {
            return .syncing(changeCount: status.pendingOperationCount)
        }
        if ["failed", "degraded"].contains(status.commitSync.state) {
            return .unavailable(message: status.commitSync.lastError?.message)
        }
        if status.draftSync.state != "idle"
            || !["idle", "queued", "syncing", "retrying"].contains(status.commitSync.state) {
            return .unavailable(message: nil)
        }

        if serverDataSource == "stale" { return .stale }
        if status.commitSync.state == "idle", submittedDraftCount > 0 {
            return .inReview(changeCount: submittedDraftCount)
        }
        return nil
    }

    func visible(in section: WorkspaceSection) -> Self? {
        if section == .reviews, case .inReview = self { return nil }
        return self
    }

    var isSyncing: Bool {
        if case .syncing = self { return true }
        return false
    }

    var symbolName: String {
        switch self {
        case .syncing: "arrow.triangle.2.circlepath"
        case .inReview: "checkmark.bubble"
        case .failed: "cloud.exclamationmark"
        case .unavailable(let message): message == nil ? "questionmark.circle" : "cloud.exclamationmark"
        case .stale: "clock.arrow.circlepath"
        }
    }

    var label: String {
        switch self {
        case .syncing(let count):
            count == 1 ? "Syncing 1 change" : count > 1 ? "Syncing \(count) changes" : "Syncing changes"
        case .inReview(let count):
            count == 1 ? "Synced · 1 change in review" : "Synced · \(count) changes in review"
        case .failed:
            "Changes haven't synced"
        case .unavailable(let message):
            message == nil ? "Sync status unavailable" : "Sync needs attention"
        case .stale:
            "Showing saved content"
        }
    }

    var detail: String {
        switch self {
        case .syncing:
            return "Your changes are syncing in the background."
        case .inReview:
            return "These changes have synced. Review and merge them to publish."
        case .failed(let count, _):
            return count == 1
                ? "One change couldn't be synced. Try again."
                : count > 1
                    ? "\(count) changes couldn't be synced. Try again."
                    : "Your changes couldn't be synced. Try again."
        case .unavailable(let message):
            return message == nil
                ? "Clumsies can't check whether your changes are synced. Try again to check."
                : "Clumsies couldn't finish syncing. Try again, or check the error details."
        case .stale:
            return "The latest content couldn't be loaded. What you see may be out of date."
        }
    }

    var errorDetails: String? {
        let message: String?
        switch self {
        case .failed(_, let value), .unavailable(let value): message = value
        case .syncing, .inReview, .stale: message = nil
        }
        return message?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false ? message : nil
    }

}

enum WorkspaceColumnLayout: Equatable {
    case sidebarDetail
    case sidebarContentDetail

    init(section: WorkspaceSection) {
        self = section == .reviews
            ? .sidebarDetail
            : .sidebarContentDetail
    }
}

private enum DocumentSyncReadiness {
    case ready
    case pending
    case failed
    case unavailable
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
    @EnvironmentObject private var daemonSync: DaemonSyncService
    @EnvironmentObject private var reviewModel: ReviewsModel
    @EnvironmentObject private var documentSessions: DocumentSessions
    let onSignOut: () -> Void
    let onOpenSettings: () -> Void
    let loadsReviewDetail: Bool
    @StateObject private var activityModel: ActivityModel
    @State private var splitVisibility: NavigationSplitViewVisibility = .all
    @State private var reviewSplitVisibility: NavigationSplitViewVisibility = .all
    @State private var activitySplitVisibility: NavigationSplitViewVisibility = .all
    @State private var showsBundleResourcePicker = false
    @State private var confirmsBundleDeletion = false
    @State private var showsSyncIssuePopover = false
    @State private var reviewNavigationPath: [ReviewRoute] = []
    @State private var workspaceSearchFocusRequest = 0
    @State private var reviewSearchQuery = ""
    @State private var reviewSearchFocusRequest = 0
    @State private var reviewFilters = ReviewListFilters()
    @State private var pendingReviewToolbarAction: ReviewMenuAction?
    @State private var pendingProjectReviewDrafts: [LocalDraft] = []
    @State private var showsProjectReviewRequest = false

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
            case .reviews:
                reviewsWorkspace
            case .sessions:
                activityWorkspace
            default:
                regularWorkspace
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if let message = workspaceFeedback.errorMessage {
                WorkspaceOperationErrorBanner(message: message) {
                    workspaceFeedback.dismissErrorMessage()
                }
            }
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
                            .help("Go Back")
                            .accessibilityLabel("Go Back")

                            Button {
                                workspaceNavigation.goForward()
                            } label: {
                                Image(systemName: "chevron.right")
                            }
                            .disabled(!workspaceNavigation.canGoForward)
                            .help("Go Forward")
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
                            .help("Add Memory")

                            Menu {
                                Button("Delete Bundle", role: .destructive) {
                                    confirmsBundleDeletion = true
                                }
                            } label: {
                                Image(systemName: "ellipsis")
                            }
                            .menuIndicator(.hidden)
                            .help("Bundle Actions")
                        }

                        if let syncToolbarPresentation {
                            switch syncToolbarPresentation {
                            case .syncing:
                                ProgressView()
                                    .controlSize(.small)
                                    .frame(width: 24, height: 24)
                                    .help(syncToolbarPresentation.label)
                                    .accessibilityLabel(syncToolbarPresentation.label)
                            case .failed, .unavailable, .stale, .inReview:
                                Button {
                                    showsSyncIssuePopover.toggle()
                                } label: {
                                    syncToolbarPresentation.icon
                                }
                                .help(syncToolbarPresentation.label)
                                .accessibilityLabel(syncToolbarPresentation.label)
                                .popover(isPresented: $showsSyncIssuePopover, arrowEdge: .top) {
                                    SyncIssuePopover(
                                        presentation: syncToolbarPresentation,
                                        store: store
                                    )
                                }
                            }
                        }

                        if showsDocumentTabs, let item = workspaceNavigation.currentItem {
                            if documentNeedsSync {
                                switch documentSyncReadiness {
                                case .pending:
                                    ProgressView()
                                        .controlSize(.small)
                                        .frame(width: 24, height: 24)
                                        .help("Saving draft changes before sync")
                                        .accessibilityLabel("Saving draft changes before sync")
                                case .failed:
                                    if daemonSync.isRetryingSync(
                                        channel: "drafts",
                                        projectId: item.draft?.projectId ?? workspaceContext.activeProjectId
                                    ) {
                                        ProgressView()
                                            .controlSize(.small)
                                            .frame(width: 24, height: 24)
                                            .help("Retrying sync")
                                            .accessibilityLabel("Retrying sync")
                                    } else {
                                        Button {
                                            Task {
                                                _ = await daemonSync.retrySync(
                                                    channel: "drafts",
                                                    projectId: item.draft?.projectId
                                                        ?? workspaceContext.activeProjectId
                                                )
                                            }
                                        } label: {
                                            Image(systemName: "arrow.clockwise")
                                        }
                                        .help("Retry sync")
                                        .accessibilityLabel("Retry sync")
                                    }
                                case .unavailable:
                                    Button {} label: {
                                        Image(systemName: "exclamationmark.triangle")
                                    }
                                    .disabled(true)
                                    .help("Draft is not available on the server yet")
                                    .accessibilityLabel("Draft is not available on the server yet")
                                case .ready:
                                    Button {
                                        memoryModel.syncDocument(item)
                                    } label: {
                                        Image(systemName: item.draft?.reconciliation == .conflicts
                                            ? "exclamationmark.triangle"
                                            : "arrow.trianglehead.2.clockwise.rotate.90")
                                    }
                                    .help(item.draft?.reconciliation == .conflicts ? "Review conflicting changes" : "Sync")
                                    .accessibilityLabel(item.draft?.reconciliation == .conflicts ? "Review conflicting changes" : "Sync")
                                }
                            }

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
                            .help("Document View")
                            .accessibilityLabel("Document View")

                        }

                        if showsMemoryContentToolbar {
                            Button {
                                memoryModel.exportMemory()
                            } label: {
                                if memoryModel.isExportingMemory {
                                    ProgressView().controlSize(.small)
                                } else {
                                    Image(systemName: "square.and.arrow.down")
                                }
                            }
                            .disabled(!memoryModel.canExportMemory(memoryModel.visibleMemoryItems))
                            .help(workspaceContext.activeProjectId == nil
                                ? "Export Organization Memory as ZIP…"
                                : "Export Project Memory as ZIP…")
                            .accessibilityLabel(workspaceContext.activeProjectId == nil
                                ? "Export Organization Memory as ZIP"
                                : "Export Project Memory as ZIP")

                            Menu {
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
                                    if canProposeOrganizationDeletion(item),
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
                                    pendingProjectReviewDrafts = activeProjectReviewDrafts
                                    showsProjectReviewRequest = true
                                }
                                .disabled(activeProjectReviewDrafts.isEmpty)
                            } label: {
                                Image(systemName: "ellipsis")
                            }
                            .menuIndicator(.hidden)
                            .help("Memory Actions")
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
                            accessibilityHelp: "Search across the current workspace",
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
        .onChange(of: syncToolbarPresentation) { _, presentation in
            if presentation == nil || presentation?.isSyncing == true {
                showsSyncIssuePopover = false
            }
        }
        .sheet(isPresented: $showsProjectReviewRequest) {
            ReviewRequestSheet(
                initialTitle: "Update \(workspaceContext.activeProject?.name ?? "project") memory",
                loadCandidates: {
                    try await reconciler.reconciliationCandidates(for: pendingProjectReviewDrafts)
                }
            ) { title, description, reconciliations in
                try await reviewModel.requestReview(
                    for: pendingProjectReviewDrafts,
                    title: title,
                    description: description,
                    reconciliations: reconciliations
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
        .onChange(of: syncToolbarPresentation) { _, presentation in
            if presentation == nil || presentation?.isSyncing == true {
                showsSyncIssuePopover = false
            }
        }
    }

    private var workspaceSearchPrompt: String {
        switch workspaceNavigation.selectedSection {
        case .memory: "Search Memory"
        case .bundles: "Search Bundles"
        case .reviews: "Search Reviews"
        case .sessions: "Search Activity"
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
            canDecideReviews: workspaceContext.canDecideReviews,
            canMergeReviews: workspaceContext.canMergeReviews,
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
            if let update = reviewModel.updates[review.id] {
                ToolbarItem(id: "review.update", placement: .automatic) {
                    ReviewUpdateToolbarButton(model: update) { detail in
                        reviewModel.endUpdate(review.id, result: detail)
                    }
                    .disabled(pendingReviewToolbarAction != nil)
                }
            }
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
                    .help(review.freshness == .behind
                        ? "Update this Review to the latest remote version before deciding"
                        : "Reject this Review")
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
                    .help(review.freshness == .behind
                        ? "Update this Review to the latest remote version before deciding"
                        : "Approve and merge this Review")
                    .accessibilityLabel("Approve and Merge Review")
                    .accessibilityIdentifier("review-toolbar-approve")
                }
            }

            if reviewToolbarOwnership.contains(.decision(.merge)) {
                ToolbarItem(id: "review.merge", placement: .automatic) {
                    Button {
                        performReviewToolbarAction(.merge)
                    } label: {
                        reviewToolbarActionLabel(
                            systemImage: "arrow.triangle.merge",
                            isPending: pendingReviewToolbarAction == .merge
                        )
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(
                        pendingReviewToolbarAction != nil
                            || !reviewModel.canPerformReviewMenuAction(.merge)
                    )
                    .help("Merge the approved changes")
                    .accessibilityLabel("Merge Review")
                    .accessibilityIdentifier("review-toolbar-merge")
                }
            }

            if reviewToolbarOwnership.contains(.decision(.resubmit)) {
                ToolbarItem(id: "review.resubmit", placement: .automatic) {
                    Button {
                        performReviewToolbarAction(.resubmit)
                    } label: {
                        reviewToolbarActionLabel(
                            systemImage: "arrow.clockwise",
                            isPending: pendingReviewToolbarAction == .resubmit
                        )
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(
                        pendingReviewToolbarAction != nil
                            || !reviewModel.canPerformReviewMenuAction(.resubmit)
                    )
                    .help("Resubmit this Review")
                    .accessibilityLabel("Resubmit Review")
                    .accessibilityIdentifier("review-toolbar-resubmit")
                }
            }
        }

        reviewUtilityToolbarContent(hasLeadingActions: reviewToolbarOwnership.hasDecisionActions)
    }

    @ToolbarContentBuilder
    private func reviewUtilityToolbarContent(hasLeadingActions: Bool) -> some ToolbarContent {
        if #available(macOS 26.0, *),
           syncToolbarPresentation != nil,
           hasLeadingActions {
            ToolbarSpacer(.fixed, placement: .automatic)
        }

        if let syncToolbarPresentation {
            ToolbarItem(id: "review.sync", placement: .automatic) {
                switch syncToolbarPresentation {
                case .syncing:
                    ProgressView()
                        .controlSize(.small)
                        .frame(width: 24, height: 24)
                        .help(syncToolbarPresentation.label)
                        .accessibilityLabel(syncToolbarPresentation.label)
                        .accessibilityIdentifier("review-toolbar-sync")
                case .failed, .unavailable, .stale, .inReview:
                    Button {
                        showsSyncIssuePopover.toggle()
                    } label: {
                        syncToolbarPresentation.icon
                    }
                    .help(syncToolbarPresentation.label)
                    .accessibilityLabel(syncToolbarPresentation.label)
                    .accessibilityIdentifier("review-toolbar-sync")
                    .popover(isPresented: $showsSyncIssuePopover, arrowEdge: .top) {
                        SyncIssuePopover(
                            presentation: syncToolbarPresentation,
                            store: store
                        )
                    }
                }
            }
        }

        if #available(macOS 26.0, *),
           syncToolbarPresentation != nil || hasLeadingActions {
            ToolbarSpacer(.fixed, placement: .automatic)
        }

        if reviewToolbarOwnership.contains(.search) {
            ToolbarItem(id: "review.search", placement: .trailingPinned) {
                ClassicSearchField(
                    text: $reviewSearchQuery,
                    prompt: "Search Reviews",
                    accessibilityIdentifier: "review-toolbar-search",
                    accessibilityHelp: "Search Reviews by title, description or author",
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
            ZStack {
                // Keep the session views mounted so their selection, divider and
                // exact scroll position survive a visit to the retrieval trace.
                HSplitView {
                    ActivitySessionList(model: activityModel)
                        .frame(minWidth: 260, idealWidth: 300, maxWidth: 380, maxHeight: .infinity)
                    ActivitySessionDetail(model: activityModel)
                        .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
                }
                .opacity(activityModel.retrievalSelection == nil ? 1 : 0)
                .allowsHitTesting(activityModel.retrievalSelection == nil)
                .accessibilityHidden(activityModel.retrievalSelection != nil)
                .disabled(activityModel.retrievalSelection != nil)

                if let selection = activityModel.retrievalSelection {
                    ActivityRetrievalDetail(
                        selection: selection,
                        daemon: workspaceContext.daemon,
                        onBack: activityModel.closeRetrieval
                    )
                    .frame(minWidth: RetrievalDiagnosticsLayout.mainPaneMinimumWidth)
                }
            }
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
            .help("Refresh Activity")
            .accessibilityLabel("Refresh Activity")
        }
    }

    @ToolbarContentBuilder
    private var navigationToolbarContent: some ToolbarContent {
        switch workspaceNavigation.selectedSection {
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
                .help("Project Settings")
                .accessibilityLabel("Project Settings")
            }
        case .bundles:
            ToolbarItem {
                Button {
                    Task { await bundleModel.createBundle() }
                } label: {
                    Image(systemName: "plus")
                }
                .help("New Bundle")
                .accessibilityLabel("New Bundle")
            }
        case .reviews:
            ToolbarItem {
                EmptyView()
            }
        case .sessions:
            ToolbarItem {
                EmptyView()
            }
        }
    }

    @ViewBuilder
    private var navigator: some View {
        switch workspaceNavigation.selectedSection {
        case .memory:
            MemoryNavigator()
        case .bundles:
            BundleNavigator()
        case .reviews:
            EmptyView()
        case .sessions:
            EmptyView()
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch workspaceNavigation.selectedSection {
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
        case .sessions:
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
        return draft.status == .open
            && draft.scope == .org
            && ReviewsModel.canRequestReview(draft)
    }

    private func canProposeOrganizationDeletion(_ item: MemoryListItem) -> Bool {
        draftStore.canEditMemory(item)
            && MemoryFileTreeMenu.canProposeOrganizationDeletion(
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
            || canProposeOrganizationDeletion(item)
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

    private var documentNeedsSync: Bool {
        guard let item = workspaceNavigation.currentItem else { return false }
        return SharedUpdateStatusPresentation.resolve(
            freshness: item.draft?.freshness,
            hasUpstreamResourceChanges: item.draft?.hasUpstreamResourceChanges == true,
            reconciliation: item.draft?.reconciliation,
            isStale: item.draft == nil
                && item.resource.map { memoryCatalog.staleResourceIds.contains($0.id) } == true
        ) != nil
    }

    private var documentSyncReadiness: DocumentSyncReadiness {
        guard let item = workspaceNavigation.currentItem else { return .ready }
        if documentSessions.isSynchronizingDocument(item.id) { return .pending }
        guard let draft = item.draft else {
            return .ready
        }
        switch draft.syncStatus {
        case .queued, .syncing, .retrying:
            return .pending
        case .failed:
            return .failed
        case .synced:
            return draft.serverId == nil ? .unavailable : .ready
        }
    }

    private var syncToolbarPresentation: SyncToolbarPresentation? {
        guard workspaceContext.activeProjectId != nil else { return nil }
        return SyncToolbarPresentation.resolve(
            status: daemonSync.runtime?.sync,
            isAvailable: daemonSync.syncStatusAvailable,
            serverDataSource: daemonSync.runtime?.serverDataSource,
            submittedDraftCount: draftStore.draftInventoryLoadState == .loaded
                ? reviewModel.submittedProjectDrafts.count : 0
        )?.visible(in: workspaceNavigation.selectedSection)
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
            unscopedTitle: "Org",
            unscopedSystemImage: "building.2",
            isLoading: workspaceContext.isSwitchingMemoryContext,
            help: "Filter Memory by Project",
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
            unscopedTitle: "All Projects",
            unscopedSystemImage: nil,
            isLoading: false,
            help: "Filter Activity by Project",
            onCreate: workspaceContext.canCreateProject ? { workspaceNavigation.presentProjectCreation() } : nil
        ) { projectId in
            Task { await model.selectProject(projectId) }
        }
    }
}

private extension SyncToolbarPresentation {
    @ViewBuilder
    var icon: some View {
        if case .inReview = self {
            DraftReviewIcon()
        } else {
            Image(systemName: symbolName)
                .foregroundStyle(tint)
        }
    }

    var tint: Color {
        switch self {
        case .syncing: .secondary
        case .inReview: Color(nsColor: .systemGreen)
        case .failed: .red
        case .unavailable: .secondary
        case .stale: .secondary
        }
    }
}

private struct WorkspaceOperationErrorBanner: View {
    let message: String
    let onDismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
                .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 2) {
                Text("Operation Failed")
                    .font(.headline)
                Text(message)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Operation failed: \(message)")

            Spacer(minLength: 16)

            Button("Copy Details") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(message, forType: .string)
            }

            Button(action: onDismiss) {
                Image(systemName: "xmark")
            }
            .buttonStyle(.plain)
            .help("Dismiss")
            .accessibilityLabel("Dismiss operation failure")
        }
        .padding(10)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }
}

private struct SyncIssuePopover: View {
    @EnvironmentObject private var draftStore: DraftStore
    @EnvironmentObject private var workspaceFeedback: WorkspaceFeedback
    let presentation: SyncToolbarPresentation
    let store: WorkspaceCoordinator
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var daemonSync: DaemonSyncService
    @EnvironmentObject private var reviewModel: ReviewsModel
    @State private var isReloading = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                presentation.icon
                Text(presentation.label)
                    .font(.headline)
            }

            Text(presentation.detail)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if case .inReview = presentation {
                let reviewIds = Set(reviewModel.submittedProjectDrafts.compactMap {
                    reviewModel.review(for: $0)?.id
                })
                ForEach(reviewModel.reviews.filter { reviewIds.contains($0.id) }) { review in
                    Button(review.title) {
                        dismiss()
                        reviewModel.openReview(review)
                    }
                    .help("View Review")
                }
                ForEach(reviewModel.submittedProjectDrafts.filter { reviewModel.review(for: $0) == nil }) { draft in
                    Button("View Review for \(draft.document.title)") {
                        dismiss()
                        Task { await reviewModel.openReview(for: draft) }
                    }
                }
            }

            if daemonSync.syncRetryErrorMessage != nil {
                Label("Sync still couldn't finish. You can try again.", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let message = daemonSync.syncRetryErrorMessage ?? presentation.errorDetails {
                DisclosureGroup("Error details") {
                    Text(message)
                        .font(.caption)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            Divider()

            HStack {
                Spacer()
                switch presentation {
                case .failed, .unavailable:
                    Button {
                        guard let projectId = workspaceContext.activeProjectId else { return }
                        Task { _ = await daemonSync.retrySync(projectId: projectId) }
                    } label: {
                        if daemonSync.isRetryingSync {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Text("Try Again")
                        }
                    }
                    .disabled(daemonSync.isRetryingSync)
                    .keyboardShortcut(.defaultAction)
                case .stale:
                    Button {
                        isReloading = true
                        Task {
                            await store.reload()
                            isReloading = false
                        }
                    } label: {
                        if isReloading {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Text("Get Latest Content")
                        }
                    }
                    .disabled(isReloading)
                    .keyboardShortcut(.defaultAction)
                case .syncing, .inReview:
                    EmptyView()
                }
            }
        }
        .padding(16)
        .frame(width: 360)
    }
}

private struct GlobalSidebar: View {
    let store: WorkspaceCoordinator
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

    private var accountDisplayName: String {
        if let displayName = workspaceContext.account?.displayName?.trimmingCharacters(in: .whitespacesAndNewlines),
           !displayName.isEmpty {
            return displayName
        }
        return workspaceContext.account?.email ?? "Account"
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
