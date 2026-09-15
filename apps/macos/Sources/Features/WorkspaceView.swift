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
    case failed(changeCount: Int, message: String?)
    case unavailable(message: String?)
    case stale

    static func resolve(
        status: DaemonSyncStatus?,
        isAvailable: Bool,
        serverDataSource: String?
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
        return nil
    }

    var isSyncing: Bool {
        if case .syncing = self { return true }
        return false
    }

    var symbolName: String {
        switch self {
        case .syncing: "arrow.triangle.2.circlepath"
        case .failed: "cloud.exclamationmark"
        case .unavailable(let message): message == nil ? "questionmark.circle" : "cloud.exclamationmark"
        case .stale: "clock.arrow.circlepath"
        }
    }

    var label: String {
        switch self {
        case .syncing(let count):
            count == 1 ? "Syncing 1 change" : count > 1 ? "Syncing \(count) changes" : "Syncing changes"
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
        case .syncing, .stale: message = nil
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
    @ObservedObject var store: WorkspaceStore
    let onSignOut: () -> Void
    let onOpenSettings: () -> Void
    let loadsReviewDetail: Bool
    @StateObject private var recallModel: RecallModel
    @State private var splitVisibility: NavigationSplitViewVisibility = .all
    @State private var reviewSplitVisibility: NavigationSplitViewVisibility = .all
    @State private var recallSplitVisibility: NavigationSplitViewVisibility = .all
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
        store: WorkspaceStore,
        onSignOut: @escaping () -> Void,
        onOpenSettings: @escaping () -> Void,
        loadsReviewDetail: Bool = true
    ) {
        self.store = store
        self.onSignOut = onSignOut
        self.onOpenSettings = onOpenSettings
        self.loadsReviewDetail = loadsReviewDetail
        _recallModel = StateObject(wrappedValue: RecallModel(daemon: store.daemon))
    }

    private var showsDocumentTabs: Bool {
        store.selectedSection == .memory
            && !store.visibleTabs.isEmpty
            && !store.showsProjectSettings
    }

    private var showsMemoryContentToolbar: Bool {
        store.selectedSection == .memory && !store.showsProjectSettings
    }

    private var documentReconciliationState: DocumentReconciliationToolbarState? {
        guard let state = store.documentReconciliationToolbarState,
              let currentItem = store.currentItem,
              state.sessionKey == store.documentSessionKey(for: currentItem) else { return nil }
        return state
    }

    private func deferSidebarExpansionUpdate(_ expanded: Bool) {
        guard store.sidebarExpanded != expanded else { return }
        DispatchQueue.main.async {
            if store.sidebarExpanded != expanded {
                store.sidebarExpanded = expanded
            }
        }
    }

    var body: some View {
        Group {
            switch store.selectedSection {
            case .reviews:
                reviewsWorkspace
            case .sessions:
                recallWorkspace
            default:
                regularWorkspace
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if let message = store.errorMessage {
                WorkspaceOperationErrorBanner(message: message) {
                    store.dismissErrorMessage()
                }
            }
        }
        .sheet(isPresented: $store.showsProjectCreation) {
            ProjectCreationSheet(store: store)
        }
        .onChange(of: store.selectedSection) { _, _ in
            DispatchQueue.main.async {
                store.searchQuery = ""
                if store.selectedSection != .memory {
                    store.showsProjectSettings = false
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
                                if let state = documentReconciliationState {
                                    store.pendingDocumentCommand = .closeReconciliation(
                                        sessionKey: state.sessionKey
                                    )
                                } else {
                                    store.goBack()
                                }
                            } label: {
                                Image(systemName: "chevron.left")
                            }
                            .disabled(
                                documentReconciliationState?.isUpdating == true
                                    || (documentReconciliationState == nil && !store.canGoBack)
                            )
                            .help(documentReconciliationState == nil ? "Go Back" : "Back to Document")
                            .accessibilityLabel(
                                documentReconciliationState == nil ? "Go Back" : "Back to Document"
                            )

                            Button {
                                store.goForward()
                            } label: {
                                Image(systemName: "chevron.right")
                            }
                            .disabled(documentReconciliationState != nil || !store.canGoForward)
                            .help("Go Forward")
                            .accessibilityLabel("Go Forward")
                        }
                    }

                    if #available(macOS 26.0, *) {
                        ToolbarSpacer(.flexible)
                    }

                    ToolbarItemGroup(placement: .trailingPinned) {
                        if store.selectedSection == .bundles, store.selectedBundle != nil {
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
                            case .failed, .unavailable, .stale:
                                Button {
                                    showsSyncIssuePopover.toggle()
                                } label: {
                                    Image(systemName: syncToolbarPresentation.symbolName)
                                        .foregroundStyle(syncToolbarPresentation.tint)
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

                        if showsDocumentTabs, let item = store.currentItem {
                            if let state = documentReconciliationState {
                                if state.isLoading || state.isUpdating {
                                    ProgressView()
                                        .controlSize(.small)
                                        .frame(width: 24, height: 24)
                                        .help(state.isLoading ? "Reviewing changes" : "Syncing")
                                        .accessibilityLabel(
                                            state.isLoading ? "Reviewing changes" : "Syncing"
                                        )
                                } else {
                                    Button {
                                        store.pendingDocumentCommand = .applyReconciliation(
                                            sessionKey: state.sessionKey
                                        )
                                    } label: {
                                        Image(systemName: "arrow.trianglehead.2.clockwise.rotate.90")
                                    }
                                    .disabled(!state.canUpdate)
                                    .help("Sync")
                                    .accessibilityLabel("Sync")
                                }
                            }

                            if documentReconciliationState == nil, documentNeedsSync {
                                switch documentSyncReadiness {
                                case .pending:
                                    ProgressView()
                                        .controlSize(.small)
                                        .frame(width: 24, height: 24)
                                        .help("Saving draft changes before sync")
                                        .accessibilityLabel("Saving draft changes before sync")
                                case .failed:
                                    if store.isRetryingSync(
                                        channel: "drafts",
                                        projectId: item.draft?.projectId ?? store.activeProjectId
                                    ) {
                                        ProgressView()
                                            .controlSize(.small)
                                            .frame(width: 24, height: 24)
                                            .help("Retrying sync")
                                            .accessibilityLabel("Retrying sync")
                                    } else {
                                        Button {
                                            Task {
                                                _ = await store.retrySync(
                                                    channel: "drafts",
                                                    projectId: item.draft?.projectId
                                                        ?? store.activeProjectId
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
                                        store.syncDocument(item)
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
                                documentReconciliationState != nil
                                    || availableDocumentModes.count < 2
                                    || store.isSynchronizingDocument(item.id)
                            )
                            .help("Document View")
                            .accessibilityLabel("Document View")

                        }

                        if showsMemoryContentToolbar {
                            Button {
                                store.exportMemory()
                            } label: {
                                if store.isExportingMemory {
                                    ProgressView().controlSize(.small)
                                } else {
                                    Image(systemName: "square.and.arrow.down")
                                }
                            }
                            .disabled(!store.canExportMemory(store.visibleMemoryItems))
                            .help(store.activeProjectId == nil
                                ? "Export Organization Memory as ZIP…"
                                : "Export Project Memory as ZIP…")
                            .accessibilityLabel(store.activeProjectId == nil
                                ? "Export Organization Memory as ZIP"
                                : "Export Project Memory as ZIP")

                            Menu {
                                if let item = store.currentItem {
                                    Button("Export File as ZIP…") {
                                        store.exportMemory([item], name: item.document.title)
                                    }
                                    .disabled(!store.canExportMemory([item]))
                                    Divider()
                                }
                                if let item = store.currentItem, hasDocumentActions(item) {
                                    if canRequestDocumentReview(item),
                                       let draft = item.draft,
                                       let sessionKey = store.documentSessionKey(for: item) {
                                        Button("Request Review") {
                                            store.pendingDocumentCommand = .requestReview(
                                                sessionKey: sessionKey,
                                                draft: draft
                                            )
                                        }
                                        .disabled(store.isSynchronizingDocument(item.id))
                                        Divider()
                                    }
                                    if canDiscardDocumentDraft(item),
                                       let draft = item.draft,
                                       let sessionKey = store.documentSessionKey(for: item) {
                                        Button("Discard Draft") {
                                            store.pendingDocumentCommand = .discardDraft(
                                                sessionKey: sessionKey,
                                                draft: draft
                                            )
                                        }
                                        .disabled(store.isSynchronizingDocument(item.id))
                                    }
                                    if canProposeOrganizationDeletion(item),
                                       let sessionKey = store.documentSessionKey(for: item) {
                                        Button(
                                            "Propose Organization Deletion",
                                            role: .destructive
                                        ) {
                                            store.pendingDocumentCommand = .moveToTrash(
                                                sessionKey: sessionKey
                                            )
                                        }
                                        .disabled(store.isSynchronizingDocument(item.id))
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
                            text: $store.searchQuery,
                            prompt: workspaceSearchPrompt,
                            accessibilityIdentifier: "workspace-toolbar-search",
                            accessibilityHelp: "Search across the current workspace",
                            focusToken: workspaceSearchFocusRequest
                        )
                    }
                }
            }
        .onAppear {
            let target: NavigationSplitViewVisibility = store.sidebarExpanded ? .all : .doubleColumn
            if splitVisibility != target {
                splitVisibility = target
            }
        }
        .onChange(of: store.workspaceSearchFocusToken) { _, _ in
            workspaceSearchFocusRequest += 1
        }
        .onChange(of: store.searchQuery) { _, query in
            guard store.selectedSection == .memory,
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
        .onChange(of: store.sidebarExpanded) { _, expanded in
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
                initialTitle: "Update \(store.activeProject?.name ?? "project") memory",
                loadCandidates: {
                    try await store.reconciliationCandidates(for: pendingProjectReviewDrafts)
                }
            ) { title, description, reconciliations in
                try await store.requestReview(
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
                ReviewListPage(
                    store: store,
                    reviews: filteredReviews,
                    searchQuery: reviewSearchQuery,
                    filters: $reviewFilters,
                    toolbarOwnership: reviewToolbarOwnership,
                    onClearFilters: {
                        reviewFilters = ReviewListFilters(status: .all)
                        reviewSearchQuery = ""
                    }
                )
                .navigationDestination(for: ReviewRoute.self) { route in
                    ReviewDetailPage(
                        store: store,
                        reviewId: route.reviewId,
                        loadsRemoteContent: loadsReviewDetail
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
            let target: NavigationSplitViewVisibility = store.sidebarExpanded ? .all : .detailOnly
            if reviewSplitVisibility != target {
                reviewSplitVisibility = target
            }

            if let routedReviewId = reviewNavigationPath.last?.reviewId,
               !store.reviews.contains(where: { $0.id == routedReviewId }) {
                reviewNavigationPath.removeAll()
            }
            if reviewNavigationPath.isEmpty,
               let reviewId = store.selectedReviewId,
               store.reviews.contains(where: { $0.id == reviewId }) {
                reviewNavigationPath = [ReviewRoute(reviewId: reviewId)]
            }
        }
        .onChange(of: reviewSplitVisibility) { _, visibility in
            let expanded = visibility != .detailOnly
            deferSidebarExpansionUpdate(expanded)
        }
        .onChange(of: store.sidebarExpanded) { _, expanded in
            let target: NavigationSplitViewVisibility = expanded ? .all : .detailOnly
            if reviewSplitVisibility != target {
                reviewSplitVisibility = target
            }
        }
        .onChange(of: reviewNavigationPath) { _, path in
            let reviewId = path.last?.reviewId
            DispatchQueue.main.async {
                if store.selectedReviewId != reviewId {
                    store.selectedReviewId = reviewId
                }
                if reviewId == nil {
                    store.reviewDecisionReadiness = nil
                }
            }
            if reviewId == nil {
                pendingReviewToolbarAction = nil
            }
        }
        .onChange(of: store.reviewSearchFocusToken) { _, _ in
            reviewNavigationPath.removeAll()
            DispatchQueue.main.async {
                reviewSearchFocusRequest += 1
            }
        }
        .onChange(of: store.selectedReviewId) { _, reviewId in
            guard store.selectedSection == .reviews else { return }
            guard let reviewId else {
                if !reviewNavigationPath.isEmpty {
                    reviewNavigationPath.removeAll()
                }
                return
            }
            guard reviewNavigationPath.last?.reviewId != reviewId else { return }
            guard store.reviews.contains(where: { $0.id == reviewId }) else { return }
            reviewNavigationPath = [ReviewRoute(reviewId: reviewId)]
        }
        .onChange(of: syncToolbarPresentation) { _, presentation in
            if presentation == nil || presentation?.isSyncing == true {
                showsSyncIssuePopover = false
            }
        }
    }

    private var workspaceSearchPrompt: String {
        switch store.selectedSection {
        case .memory: "Search Memory"
        case .bundles: "Search Bundles"
        case .reviews: "Search Reviews"
        case .sessions: "Search Activity"
        }
    }

    private var filteredReviews: [ReviewRecord] {
        let byFilters = store.reviews.filter(reviewFilters.matches)
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
            canDecideReviews: store.canDecideReviews,
            canMergeReviews: store.canMergeReviews,
            isAuthor: review.map(store.isReviewAuthor) ?? false
        )
    }

    private var selectedReviewForToolbar: ReviewRecord? {
        guard let reviewId = store.selectedReviewId else { return nil }
        return store.reviews.first { $0.id == reviewId }
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
                            || !store.canPerformReviewMenuAction(.reject)
                    )
                    .help(review.freshness == .behind
                        ? "Review the latest shared changes before deciding"
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
                    .buttonStyle(.borderedProminent)
                    .disabled(
                        pendingReviewToolbarAction != nil
                            || !store.canPerformReviewMenuAction(.approve)
                    )
                    .help(review.freshness == .behind
                        ? "Review the latest shared changes before deciding"
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
                            || !store.canPerformReviewMenuAction(.merge)
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
                            || !store.canPerformReviewMenuAction(.resubmit)
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
                case .failed, .unavailable, .stale:
                    Button {
                        showsSyncIssuePopover.toggle()
                    } label: {
                        Image(systemName: syncToolbarPresentation.symbolName)
                            .foregroundStyle(syncToolbarPresentation.tint)
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
            await store.performReviewMenuAction(action)
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

    private var recallWorkspace: some View {
        NavigationSplitView(columnVisibility: $recallSplitVisibility) {
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
                    RecallSessionList(model: recallModel)
                        .frame(minWidth: 260, idealWidth: 300, maxWidth: 380, maxHeight: .infinity)
                    RecallSessionDetail(model: recallModel)
                        .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
                }
                .opacity(recallModel.retrievalSelection == nil ? 1 : 0)
                .allowsHitTesting(recallModel.retrievalSelection == nil)
                .accessibilityHidden(recallModel.retrievalSelection != nil)
                .disabled(recallModel.retrievalSelection != nil)

                if let selection = recallModel.retrievalSelection {
                    RecallRetrievalDetail(
                        selection: selection,
                        daemon: store.daemon,
                        onBack: recallModel.closeRetrieval
                    )
                    .frame(minWidth: RetrievalDiagnosticsLayout.mainPaneMinimumWidth)
                }
            }
            .toolbar {
                if recallModel.retrievalSelection == nil {
                    recallToolbarContent
                }
            }
        }
        .onAppear {
            let target: NavigationSplitViewVisibility = store.sidebarExpanded ? .all : .detailOnly
            if recallSplitVisibility != target {
                recallSplitVisibility = target
            }
            if recallModel.sessions.isEmpty {
                Task { await recallModel.load() }
            }
        }
        .onChange(of: recallSplitVisibility) { _, visibility in
            deferSidebarExpansionUpdate(visibility != .detailOnly)
        }
        .onChange(of: store.sidebarExpanded) { _, expanded in
            let target: NavigationSplitViewVisibility = expanded ? .all : .detailOnly
            if recallSplitVisibility != target {
                recallSplitVisibility = target
            }
        }
    }

    @ToolbarContentBuilder
    private var recallToolbarContent: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            ActivityProjectFilter(store: store, model: recallModel)
        }

        if #available(macOS 26.0, *) {
            ToolbarSpacer(.flexible, placement: .automatic)
        }

        ToolbarItem(placement: .trailingPinned) {
            Button {
                Task { await recallModel.load() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .disabled(recallModel.isLoading)
            .help("Refresh Activity")
            .accessibilityLabel("Refresh Activity")
        }
    }

    @ToolbarContentBuilder
    private var navigationToolbarContent: some ToolbarContent {
        switch store.selectedSection {
        case .memory:
            ToolbarItem(placement: .navigation) {
                MemoryProjectFilter(store: store)
            }

            ToolbarItem {
                Button {
                    store.showsProjectSettings.toggle()
                } label: {
                    Image(systemName: "gearshape")
                }
                .disabled(store.activeProjectId == nil)
                .help("Project Settings")
                .accessibilityLabel("Project Settings")
            }
        case .bundles:
            ToolbarItem {
                Button {
                    Task { await store.createBundle() }
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
        switch store.selectedSection {
        case .memory:
            MemoryNavigator(store: store)
        case .bundles:
            BundleNavigator(store: store)
        case .reviews:
            EmptyView()
        case .sessions:
            EmptyView()
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch store.selectedSection {
        case .memory:
            if store.projects.isEmpty, !store.resources.contains(where: { $0.scope == .org }) {
                ProjectUnavailableView(store: store)
            } else if store.showsProjectSettings, let projectId = store.activeProjectId {
                ProjectSettingsView(store: store, projectId: projectId)
            } else {
                MemoryMainPane(store: store)
            }
        case .bundles:
            BundleDetail(
                store: store,
                showsResourcePicker: $showsBundleResourcePicker,
                confirmsDeletion: $confirmsBundleDeletion
            )
        case .reviews:
            EmptyView()
        case .sessions:
            EmptyView()
        }
    }

    private var availableDocumentModes: [WorkbenchTabMode] {
        if store.currentItem?.draft?.documentBaselineAvailable == false {
            return [.diff]
        }
        return store.currentItem?.supportsMarkdownPreview == true
            ? [.preview, .source, .diff]
            : [.source, .diff]
    }

    private func canRequestDocumentReview(_ item: MemoryListItem) -> Bool {
        guard store.activeProjectId != nil,
              let draft = item.draft else {
            return false
        }
        return draft.status == .open
            && draft.scope == .org
            && WorkspaceStore.canRequestReview(draft)
    }

    private func canProposeOrganizationDeletion(_ item: MemoryListItem) -> Bool {
        store.canEditMemory(item)
            && MemoryFileTreeMenu.canProposeOrganizationDeletion(
                item,
                inOrgView: store.activeProjectId == nil
            )
    }

    private func canDiscardDocumentDraft(_ item: MemoryListItem) -> Bool {
        store.activeProjectId != nil && item.draft != nil
    }

    private func hasDocumentActions(_ item: MemoryListItem) -> Bool {
        canRequestDocumentReview(item)
            || canDiscardDocumentDraft(item)
            || canProposeOrganizationDeletion(item)
    }

    private var activeProjectReviewDrafts: [LocalDraft] {
        WorkspaceStore.reviewableProjectDrafts(
            store.drafts,
            projectId: store.activeProjectId
        )
    }

    private var documentMode: Binding<WorkbenchTabMode> {
        Binding(
            get: {
                let mode = store.currentTabMode ?? .preview
                return availableDocumentModes.contains(mode) ? mode : .source
            },
            set: { store.switchDocumentMode($0) }
        )
    }

    private var documentNeedsSync: Bool {
        guard let item = store.currentItem else { return false }
        return SharedUpdateStatusPresentation.resolve(
            freshness: item.draft?.freshness,
            hasUpstreamResourceChanges: item.draft?.hasUpstreamResourceChanges == true,
            reconciliation: item.draft?.reconciliation,
            isStale: item.draft == nil
                && item.resource.map { store.staleResourceIds.contains($0.id) } == true
        ) != nil
    }

    private var documentSyncReadiness: DocumentSyncReadiness {
        guard let item = store.currentItem else { return .ready }
        if store.isSynchronizingDocument(item.id) { return .pending }
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
        guard store.activeProjectId != nil else { return nil }
        return SyncToolbarPresentation.resolve(
            status: store.runtime?.sync,
            isAvailable: store.syncStatusAvailable,
            serverDataSource: store.runtime?.serverDataSource
        )
    }

}

private struct MemoryProjectFilter: View {
    @ObservedObject var store: WorkspaceStore
    @State private var showsOrganizationProjects = false

    var body: some View {
        ProjectFilterMenu(
            projects: store.projects,
            selectedProjectId: store.activeProjectId,
            unscopedTitle: "Org",
            unscopedSystemImage: "building.2",
            isLoading: store.isSwitchingMemoryContext,
            help: "Filter Memory by Project",
            onCreate: store.canCreateProject ? { store.presentProjectCreation() } : nil,
            onBrowseOrganization: store.canAdministerOrganization ? { showsOrganizationProjects = true } : nil
        ) { projectId in
            if let projectId {
                Task { await store.selectProject(projectId) }
            } else {
                Task { await store.showOrgMemory() }
            }
        }
        .sheet(isPresented: $showsOrganizationProjects) {
            OrganizationProjectsView(store: store)
        }
    }
}

private struct ActivityProjectFilter: View {
    @ObservedObject var store: WorkspaceStore
    @ObservedObject var model: RecallModel

    var body: some View {
        ProjectFilterMenu(
            projects: store.projects,
            selectedProjectId: model.selectedProjectId,
            unscopedTitle: "All Projects",
            unscopedSystemImage: nil,
            isLoading: model.isLoading,
            help: "Filter Activity by Project",
            onCreate: store.canCreateProject ? { store.presentProjectCreation() } : nil
        ) { projectId in
            Task { await model.selectProject(projectId) }
        }
    }
}

private extension SyncToolbarPresentation {
    var tint: Color {
        switch self {
        case .syncing: .secondary
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
    let presentation: SyncToolbarPresentation
    @ObservedObject var store: WorkspaceStore
    @State private var isReloading = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Image(systemName: presentation.symbolName)
                    .foregroundStyle(presentation.tint)
                Text(presentation.label)
                    .font(.headline)
            }

            Text(presentation.detail)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if store.syncRetryErrorMessage != nil {
                Label("Sync still couldn't finish. You can try again.", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let message = store.syncRetryErrorMessage ?? presentation.errorDetails {
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
                        guard let projectId = store.activeProjectId else { return }
                        Task { _ = await store.retrySync(projectId: projectId) }
                    } label: {
                        if store.isRetryingSync {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Text("Try Again")
                        }
                    }
                    .disabled(store.isRetryingSync)
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
                case .syncing:
                    EmptyView()
                }
            }
        }
        .padding(16)
        .frame(width: 360)
    }
}

private struct GlobalSidebar: View {
    @ObservedObject var store: WorkspaceStore
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
                    Text(store.organization?.name ?? "Clumsies Lab")
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
        NativeAccountMenu(
            account: store.account,
            displayName: accountDisplayName,
            onOpenSettings: onOpenSettings,
            onSignOut: onSignOut
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var accountDisplayName: String {
        if let displayName = store.account?.displayName?.trimmingCharacters(in: .whitespacesAndNewlines),
           !displayName.isEmpty {
            return displayName
        }
        return store.account?.email ?? "Account"
    }

    private var selection: Binding<GlobalSidebarDestination?> {
        Binding(
            get: { .section(store.selectedSection) },
            set: { destination in
                guard let destination else { return }
                if case .section(let section) = destination {
                    DispatchQueue.main.async {
                        store.selectedSection = section
                        store.selectedItemId = nil
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
