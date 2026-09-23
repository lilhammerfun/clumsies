import AppKit
import CryptoKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class WorkspaceNavigationTests: XCTestCase {
    func testMemoryWorkspaceTitleIsMemory() {
        XCTAssertEqual(WorkspaceSection.memory.title, "Memory")
    }

    func testSessionWorkspaceTitleIsActivity() {
        XCTAssertEqual(WorkspaceSection.sessions.title, "Activity")
    }

    func testAdministrationMutationsRequireAdminWriteAndFreshLoadedData() {
        XCTAssertTrue(AdministrationModel.administrationMutationAllowed(
            capabilities: ["admin:write"],
            hasSnapshot: true,
            isStale: false
        ))
        XCTAssertFalse(AdministrationModel.administrationMutationAllowed(
            capabilities: [],
            hasSnapshot: true,
            isStale: false
        ))
        XCTAssertFalse(AdministrationModel.administrationMutationAllowed(
            capabilities: ["admin:write"],
            hasSnapshot: false,
            isStale: false
        ))
        XCTAssertFalse(AdministrationModel.administrationMutationAllowed(
            capabilities: ["admin:write"],
            hasSnapshot: true,
            isStale: true
        ))
    }

    func testRecallDeliveryLabelsExplainInternalActions() {
        XCTAssertEqual(recallFragment(action: "add").deliveryTitle, "Sent to agent")
        XCTAssertEqual(recallFragment(action: "replace").deliveryTitle, "Updated for agent")
        XCTAssertEqual(recallFragment(action: "reuse").deliveryTitle, "Already available")
        XCTAssertNil(recallFragment(action: "unknown").deliveryTitle)
    }

    func testRecallFragmentIdentityUsesTheChunkUnitKey() {
        let first = recallFragment(action: "add", unitKey: "memory-1#0")
        let second = recallFragment(action: "add", unitKey: "memory-1#1")

        XCTAssertNotEqual(first.id, second.id)
    }

    func testActivityTitleFallsBackToFirstUserRequest() {
        let session = RecallSession(
            host: .codex,
            sessionId: "session-1",
            title: nil,
            workspaceRoot: "/repo",
            createdAt: nil,
            tasks: [
                RecallTask(
                    messageId: "message-1",
                    text: "Explain the memory retrieval design",
                    time: nil,
                    activations: []
                )
            ]
        )

        XCTAssertEqual(session.activityDisplayTitle, "Explain the memory retrieval design")
    }

    func testSessionIdentityIncludesItsHost() {
        let dsh = RecallSession(
            host: .dsh,
            sessionId: "shared-id",
            title: nil,
            workspaceRoot: "/repo",
            createdAt: nil,
            tasks: []
        )
        let codex = RecallSession(
            host: .codex,
            sessionId: "shared-id",
            title: nil,
            workspaceRoot: "/repo",
            createdAt: nil,
            tasks: []
        )

        XCTAssertNotEqual(dsh.id, codex.id)
    }

    func testActivityOpensTheExactRetrievalAndReturnsToItsSession() {
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.activity.unused"))
        let first = recallActivation(runId: "run-1", callId: "call-1")
        let second = recallActivation(runId: "run-2", callId: "call-2")
        let task = RecallTask(messageId: "request-1", text: "Check login", time: nil, activations: [first, second])
        let session = RecallSession(host: .codex, sessionId: "session-1", title: "Login", workspaceRoot: "/repo", createdAt: nil, tasks: [task])
        model.selectedSessionId = session.id

        model.openRetrieval(session: session, activation: second)
        XCTAssertEqual(model.retrievalSelection?.runId, "run-2")
        XCTAssertEqual(model.retrievalSelection?.sessionId, session.id)

        model.closeRetrieval()
        XCTAssertNil(model.retrievalSelection)
        XCTAssertEqual(model.selectedSessionId, session.id)

        model.openRetrieval(session: session, activation: first)
        XCTAssertEqual(model.retrievalSelection?.runId, "run-1")
        model.selectedSessionId = "codex:another-session"
        XCTAssertNil(model.retrievalSelection)
        model.openRetrieval(session: session, activation: first)
        XCTAssertNil(model.retrievalSelection)

        model.selectedSessionId = session.id
        model.openRetrieval(session: session, activation: recallActivation(runId: nil))
        XCTAssertNil(model.retrievalSelection)
    }

    func testActivityChunkPreviewStaysCompactForLongSource() {
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.activity.unused"))
        func previewHeight(_ content: String) -> CGFloat {
            let view = NSHostingView(rootView: ActivityFragmentRow(
                fragment: recallFragment(action: "add", content: content),
                workspaceRoot: "/repo",
                runId: nil,
                model: model
            ).frame(width: 640))
            view.layoutSubtreeIfNeeded()
            return view.fittingSize.height
        }
        let short = previewHeight("A short paragraph.")
        let long = previewHeight((1...20).map { "Paragraph \($0): **recorded memory**, shown without opening another page." }.joined(separator: "\n\n"))
        XCTAssertLessThan(long, short + 60, "Collapsed source previews must stay within three lines.")
    }

    private func recallActivation(runId: String?, callId: String = "call-1") -> RecallActivation {
        RecallActivation(
            toolName: "memory", callId: callId, query: "same query", state: nil,
            time: nil, runId: runId, runStatus: "succeeded", fragments: [], resultError: nil
        )
    }

    private func recallFragment(
        action: String?,
        unitKey: String = "memory-1#0",
        content: String = "Example"
    ) -> RecallFragment {
        RecallFragment(
            action: action,
            unitKey: unitKey,
            resourceId: "memory-1",
            scope: .project,
            path: "memory/example.md",
            headingPath: ["Example"],
            content: content,
            finalRank: 1,
            truncated: false
        )
    }

    func testReviewsUseSidebarWithPushNavigatedDetail() {
        XCTAssertEqual(WorkspaceColumnLayout(section: .reviews), .sidebarDetail)
        XCTAssertEqual(WorkspaceColumnLayout(section: .inbox), .sidebarDetail)
        XCTAssertEqual(WorkspaceColumnLayout(section: .dashboard), .sidebarDetail)

        for section in [
            WorkspaceSection.memory,
            .bundles,
            .sessions,
        ] {
            XCTAssertEqual(
                WorkspaceColumnLayout(section: section),
                .sidebarContentDetail
            )
        }
    }

    func testReviewStatusFilterMatchesByStatus() {
        let all = ReviewStatusFilter.allCases
        let open = reviewRecord(status: "open")
        let historicalApproved = reviewRecord(status: "approved")
        XCTAssertTrue(ReviewStatusFilter.open.matches(open))
        XCTAssertTrue(ReviewStatusFilter.all.matches(open))
        XCTAssertEqual(ReviewStatusFilter.open.count(in: [open]), 1)
        XCTAssertEqual(ReviewStatusFilter.all.count(in: [open]), 1)
        XCTAssertNil(ReviewStatusFilter(rawValue: "approved"))
        XCTAssertTrue(ReviewStatusFilter.all.matches(historicalApproved))
        XCTAssertEqual(all.count, 4)
    }

    func testSubmittedMemoryOpensItsBatchReviewUsingServerDraftIdentity() async throws {
        let store = WorkspaceCoordinator()
        var review = reviewRecord(status: "open", id: "batch-review", projectId: "project")
        review.draftIds = ["draft", "server-second"]
        store.reviews.replaceReview(with: review)
        let submitted = localDraft(
            id: "second", targetId: "memory", scope: .org, status: .submitted
        )

        let linked = try XCTUnwrap(store.reviews.review(for: submitted))
        XCTAssertEqual(linked.id, "batch-review")
        store.reviews.selectedReviewId = "previous-review"
        await store.reviews.openReview(for: submitted)
        XCTAssertEqual(store.reviews.selectedReviewId, "batch-review")
        XCTAssertEqual(store.navigation.selectedSection, .reviews)
        XCTAssertNil(store.reviews.review(for: localDraft(
            id: "second", targetId: "memory", projectId: "other", scope: .org, status: .submitted
        )))
        XCTAssertNil(store.reviews.review(for: localDraft(id: "second", targetId: "memory", scope: .org)))

        var merged = reviewRecord(status: "merged", id: "batch-review", projectId: "project")
        merged.draftIds = review.draftIds
        store.reviews.replaceReview(with: merged)
        XCTAssertNil(store.reviews.review(for: submitted))
    }

    func testReviewListFiltersDefaultToOpenAndCombineAuthorAndProject() {
        let reviews = [
            reviewRecord(status: "open", id: "alice-p1", projectId: "p1", authorId: "alice"),
            reviewRecord(status: "open", id: "alice-p2", projectId: "p2", authorId: "alice"),
            reviewRecord(status: "open", id: "bob-p1", projectId: "p1", authorId: "bob"),
            reviewRecord(status: "merged", id: "merged", projectId: "p1", authorId: "alice"),
        ]
        var filters = ReviewListFilters()

        XCTAssertEqual(filters.status, .open)
        XCTAssertNil(filters.authorId)
        XCTAssertNil(filters.projectId)
        XCTAssertEqual(reviews.filter(filters.matches).map(\.id), ["alice-p1", "alice-p2", "bob-p1"])

        filters.authorId = "alice"
        XCTAssertEqual(reviews.filter(filters.matches).map(\.id), ["alice-p1", "alice-p2"])

        filters.projectId = "p1"
        XCTAssertEqual(reviews.filter(filters.matches).map(\.id), ["alice-p1"])

        filters.status = .merged
        XCTAssertEqual(reviews.filter(filters.matches).map(\.id), ["merged"])
    }

    func testReviewQueueStatePrioritizesDecisionBlockersAndViewerAction() {
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(
                    status: "merged",
                    freshness: .behind,
                    reconciliation: .conflicts
                ),
                isAuthor: false,
                canMerge: true
            ),
            .init(
                title: "Merged",
                symbolName: "arrow.triangle.merge",
                tone: .done,
                isQueueSignal: false
            )
        )
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(
                    status: "open",
                    freshness: .behind,
                    reconciliation: .conflicts
                ),
                isAuthor: false,
                canMerge: false
            ).title,
            "Conflict"
        )
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(status: "open", reconciliation: .conflicts),
                isAuthor: false,
                canMerge: false
            ).title,
            "Needs Review"
        )
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(status: "approved", freshness: .behind),
                isAuthor: false,
                canMerge: true
            ).title,
            "Checking…"
        )
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(status: "approved", freshness: .behind),
                isAuthor: true,
                canMerge: true
            ).title,
            "Checking…"
        )
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(status: "open"),
                isAuthor: false,
                canMerge: false
            ).title,
            "Needs Review"
        )
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(status: "approved", approvedResultHash: "result"),
                isAuthor: false,
                canMerge: true
            ).title,
            "Ready to Merge"
        )
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(status: "approved"),
                isAuthor: false,
                canMerge: true
            ).title,
            "Approved"
        )
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(status: "rejected"),
                isAuthor: true,
                canMerge: false
            ).title,
            "Resubmit"
        )
        XCTAssertEqual(
            ReviewQueueStatePresentation.resolve(
                review: reviewRecord(status: "rejected"),
                isAuthor: false,
                canMerge: false
            ).title,
            "Awaiting Author"
        )
    }

    func testReviewDecisionReadinessIsBoundToTheRenderedVersion() {
        let reviewId = "review-versioned"
        let rendered = reviewRecord(status: "open", id: reviewId, version: 7)
        let readiness = ReviewDecisionReadiness(review: rendered)

        XCTAssertTrue(readiness.matches(rendered))
        XCTAssertFalse(readiness.matches(reviewRecord(
            status: "open",
            id: reviewId,
            version: 8
        )))
        XCTAssertFalse(readiness.matches(reviewRecord(
            status: "rejected",
            id: reviewId,
            version: 7
        )))
        XCTAssertFalse(readiness.matches(reviewRecord(
            status: "open",
            freshness: .behind,
            id: reviewId,
            version: 7,
            currentCommitId: "commit-new"
        )))
    }

    func testMergeActionRequiresAnImmutableApprovedResult() {
        XCTAssertFalse(ReviewMenuAction.merge.isAvailable(
            for: reviewRecord(status: "approved"),
            canDecideReviews: true,
            canMergeReviews: true,
            isAuthor: false
        ))
        XCTAssertFalse(ReviewMenuAction.merge.isAvailable(
            for: reviewRecord(status: "approved", approvedResultHash: ""),
            canDecideReviews: true,
            canMergeReviews: true,
            isAuthor: false
        ))
        XCTAssertTrue(ReviewMenuAction.merge.isAvailable(
            for: reviewRecord(status: "approved", approvedResultHash: "sha256:result"),
            canDecideReviews: true,
            canMergeReviews: true,
            isAuthor: false
        ))
    }

    func testReviewDecisionActionsRequireOrganizationAuthorityCapability() {
        let review = reviewRecord(status: "open")

        XCTAssertFalse(ReviewMenuAction.approve.isAvailable(
            for: review,
            canDecideReviews: false,
            canMergeReviews: false,
            isAuthor: false
        ))
        XCTAssertFalse(ReviewMenuAction.approve.isAvailable(
            for: review,
            canDecideReviews: true,
            canMergeReviews: false,
            isAuthor: false
        ))
        XCTAssertTrue(ReviewMenuAction.approve.isAvailable(
            for: review,
            canDecideReviews: true,
            canMergeReviews: true,
            isAuthor: false
        ))
        XCTAssertTrue(ReviewMenuAction.reject.isAvailable(
            for: review,
            canDecideReviews: true,
            canMergeReviews: false,
            isAuthor: false
        ))
    }

    func testReviewListContentStateDistinguishesLoadingAndEmptyQueues() {
        XCTAssertEqual(
            ReviewListContentState.resolve(loadState: .loading, totalCount: 0, visibleCount: 0),
            .loading
        )
        XCTAssertEqual(
            ReviewListContentState.resolve(loadState: .failed("offline"), totalCount: 0, visibleCount: 0),
            .failed
        )
        XCTAssertEqual(
            ReviewListContentState.resolve(loadState: .loading, totalCount: 2, visibleCount: 0),
            .loading
        )
        XCTAssertEqual(
            ReviewListContentState.resolve(loadState: .loaded, totalCount: 0, visibleCount: 0),
            .empty
        )
        XCTAssertEqual(
            ReviewListContentState.resolve(loadState: .loaded, totalCount: 2, visibleCount: 0),
            .filteredEmpty
        )
        XCTAssertEqual(
            ReviewListContentState.resolve(loadState: .loading, totalCount: 2, visibleCount: 1),
            .content
        )
    }

    func testReviewToolbarOwnershipAcrossListAndOpenDetail() {
        let review = reviewRecord(status: "open")
        XCTAssertEqual(
            ReviewToolbarOwnership.resolve(
                surface: .list,
                review: review,
                canDecideReviews: false,
                canMergeReviews: false,
                isAuthor: false
            ).items,
            [.filter, .search]
        )
        XCTAssertEqual(
            ReviewToolbarOwnership.resolve(
                surface: .detail,
                review: review,
                canDecideReviews: true,
                canMergeReviews: true,
                isAuthor: false
            ).items,
            [.decision(.reject), .decision(.approve)]
        )
    }

    func testMemoryToolbarNavigationAndMoreActionsDoNotRequireAnOpenDocument() throws {
        let macOSRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/Workspace/WorkspaceView.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(source.contains(
            "if showsMemoryContentToolbar {\n                        ToolbarItemGroup {"
        ))
        XCTAssertTrue(source.contains(
            "if showsMemoryContentToolbar {\n                            Menu {"
        ))
        XCTAssertTrue(source.contains("Export Organization Memory as ZIP…"))
        XCTAssertTrue(source.contains("Export Project Memory as ZIP…"))
        XCTAssertTrue(source.contains(".disabled(!memoryModel.canExportMemory(memoryModel.visibleMemoryItems))"))
        XCTAssertTrue(source.contains(".toolbarHelp(String(localized: \"Memory Actions\"))"))
        XCTAssertTrue(source.contains("Request Review for All Project Changes…"))
        XCTAssertTrue(source.contains(".disabled(activeProjectReviewDrafts.isEmpty)"))
    }

    func testProjectReviewCollectsEveryCandidateBeforeOneSubmission() throws {
        let macOSRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let workspaceSource = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/Workspace/WorkspaceView.swift"),
            encoding: .utf8
        )
        let reviewSource = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/Reviews/ReviewRequestSheet.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(workspaceSource.contains(
            "reconciler.reconciliationCandidates(for: request.drafts)"
        ))
        XCTAssertTrue(reviewSource.contains("Text(\"Update and Request Review\")"))
        XCTAssertTrue(reviewSource.contains("resolvedStatesByCandidateId[candidate.candidateId]"))
        XCTAssertTrue(reviewSource.contains(".id(candidate.candidateId)"))
    }

    func testWorkspaceSearchCommandRequestsToolbarFocus() {
        let store = WorkspaceCoordinator()
        let initialFocusToken = store.navigation.workspaceSearchFocusToken

        store.navigation.focusWorkspaceSearch()
        let firstFocusToken = store.navigation.workspaceSearchFocusToken

        XCTAssertNotEqual(firstFocusToken, initialFocusToken)

        store.navigation.focusWorkspaceSearch()

        XCTAssertNotEqual(store.navigation.workspaceSearchFocusToken, firstFocusToken)
    }

    func testWorkspaceSearchFiltersMemoryContent() {
        let architecture = item(
            path: "guides/architecture.md",
            body: "Recall pipeline",
            kind: .workflows
        )
        let notes = item(path: "notes.md")
        let items = [architecture, notes]

        XCTAssertEqual(MemoryTreeProjection.filterMemoryItems(items, query: "  "), items)
        XCTAssertEqual(
            MemoryTreeProjection.filterMemoryItems(items, query: "RECALL").map(\.id),
            [architecture.id]
        )
        XCTAssertEqual(
            MemoryTreeProjection.filterMemoryItems(items, query: "workflow").map(\.id),
            [architecture.id]
        )
        XCTAssertTrue(MemoryTreeProjection.filterMemoryItems(items, query: "missing").isEmpty)
    }

    func testWorkspaceSearchFiltersBundles() {
        let release = PersonalBundle(
            id: "release",
            name: "Release",
            description: "Production checklist",
            resourceIds: [],
            revision: 1,
            updatedAt: "2026-08-31T00:00:00Z"
        )
        let onboarding = PersonalBundle(
            id: "onboarding",
            name: "Onboarding",
            description: "Starter context",
            resourceIds: [],
            revision: 1,
            updatedAt: "2026-08-31T00:00:00Z"
        )
        let bundles = [release, onboarding]

        XCTAssertEqual(BundleStore.filterBundles(bundles, query: "  "), bundles)
        XCTAssertEqual(
            BundleStore.filterBundles(bundles, query: "PRODUCTION").map(\.id),
            [release.id]
        )
        XCTAssertTrue(BundleStore.filterBundles(bundles, query: "missing").isEmpty)
    }

    func testReviewDetailRouteCarriesOnlyTheStableReviewId() {
        let route = ReviewRoute(reviewId: "review-0123456789abcdef")

        XCTAssertEqual(route.reviewId, "review-0123456789abcdef")
    }

    func testReviewStatusFiltersKeepServerStatusOrderAndTitles() {
        XCTAssertEqual(
            ReviewStatusFilter.allCases.map(\.rawValue),
            ["open", "rejected", "merged", "all"]
        )
        XCTAssertEqual(
            ReviewStatusFilter.allCases.map(\.title),
            ["Open", "Rejected", "Merged", "All"]
        )
    }

    func testBackAndForwardFollowTabSelectionHistory() {
        let store = WorkspaceCoordinator()
        let first = tab(itemId: "first")
        let second = tab(itemId: "second")
        store.navigation.tabs = [first, second]
        store.navigation.activeTabId = first.id

        store.navigation.selectTab(second)

        XCTAssertEqual(store.navigation.activeTabId, second.id)
        XCTAssertTrue(store.navigation.canGoBack)
        XCTAssertFalse(store.navigation.canGoForward)

        store.navigation.goBack()

        XCTAssertEqual(store.navigation.activeTabId, first.id)
        XCTAssertFalse(store.navigation.canGoBack)
        XCTAssertTrue(store.navigation.canGoForward)

        store.navigation.goForward()

        XCTAssertEqual(store.navigation.activeTabId, second.id)
        XCTAssertTrue(store.navigation.canGoBack)
        XCTAssertFalse(store.navigation.canGoForward)
    }

    func testNavigationHistoryUsesTheVisibleTabWhenStoredActiveTabIsFromAnotherScope() {
        let store = WorkspaceCoordinator()
        let hiddenTab = tab(itemId: "other-project", section: .memory, projectId: "other-project")
        let firstLocalTab = tab(itemId: "local-first", section: .memory, projectId: "project")
        let secondLocalTab = tab(itemId: "local-second", section: .memory, projectId: "project")
        store.navigation.tabs = [hiddenTab, firstLocalTab, secondLocalTab]
        store.navigation.selectedSection = .memory
        store.context.activeProjectId = "project"
        store.navigation.activeTabId = hiddenTab.id

        store.navigation.selectTab(firstLocalTab)
        store.navigation.goBack()

        XCTAssertEqual(store.navigation.activeTabId, secondLocalTab.id)
        XCTAssertEqual(store.navigation.selectedItemId, secondLocalTab.itemId)
    }

    func testProjectSelectionSideEffectsAreSerialized() async {
        let gate = ProjectSelectionSideEffectGate()
        let releaseFirst = WorkspaceNavigationTestLatch()
        let firstEntered = expectation(description: "first selection entered daemon side effect")
        let secondRequested = expectation(description: "second selection requested daemon side effect")
        var events: [String] = []

        let first = Task { @MainActor in
            await gate.run {
                events.append("first-start")
                firstEntered.fulfill()
                await releaseFirst.wait()
                events.append("first-end")
            }
        }
        await fulfillment(of: [firstEntered], timeout: 1)

        let second = Task { @MainActor in
            secondRequested.fulfill()
            await gate.run {
                events.append("second")
            }
        }
        await fulfillment(of: [secondRequested], timeout: 1)
        await Task.yield()
        XCTAssertEqual(events, ["first-start"])

        await releaseFirst.open()
        await first.value
        await second.value
        XCTAssertEqual(events, ["first-start", "first-end", "second"])
    }

    func testOpeningMarkdownDefaultsToPreview() {
        let store = WorkspaceCoordinator()

        store.navigation.open(item(path: "context/architecture.md"))

        XCTAssertEqual(store.navigation.activeVisibleTab?.mode, .preview)
    }

    func testOpeningPlainTextDefaultsToSource() {
        let store = WorkspaceCoordinator()

        store.navigation.open(item(path: "context/notes.txt"))

        XCTAssertEqual(store.navigation.activeVisibleTab?.mode, .source)
    }

    func testDocumentTabIdentityIsStableAcrossModes() {
        var tab = WorkbenchTab(
            section: .memory,
            projectId: "project",
            itemId: "memory",
            mode: .preview,
            title: "Memory"
        )
        let previewId = tab.id

        tab.mode = .diff

        XCTAssertEqual(tab.id, previewId)
    }

    func testOpeningAndSwitchingModesKeepsOneTabPerDocument() {
        let store = WorkspaceCoordinator()
        let document = item(path: "context/architecture.md")

        store.navigation.open(document)
        let stableId = store.navigation.activeTabId
        store.navigation.open(document, mode: .source)
        store.navigation.open(document)

        XCTAssertEqual(store.navigation.activeVisibleTab?.mode, .source)

        store.navigation.switchDocumentMode(.diff)

        XCTAssertEqual(store.navigation.tabs.count, 1)
        XCTAssertEqual(store.navigation.activeTabId, stableId)
        XCTAssertEqual(store.navigation.activeVisibleTab?.mode, .diff)
    }

    func testStaleResourcePlanRejectsAnOlderDaemonCheckout() {
        let displayed = projectResource(
            id: "memory",
            path: "memory.md",
            hash: "hash-new",
            commitId: "commit-new"
        )
        let checkout = projectCheckout(
            commitId: "commit-old",
            resources: [checkoutResource(id: "memory", path: "memory.md", hash: "hash-old")]
        )

        let plan = MemorySyncPlan.staleResourcePlan(
            displayedResources: [displayed],
            projectName: "Project",
            observedProjectRefCommitId: "commit-new",
            authoritativeCommitId: "commit-new",
            serverCursor: "commit-old",
            checkout: checkout
        )

        XCTAssertNil(plan)
    }

    func testStaleCachedAuthoritativeRefCannotBuildRollbackPlan() {
        let displayed = projectResource(
            id: "memory",
            path: "memory.md",
            hash: "hash-new",
            commitId: "commit-new"
        )
        let checkout = projectCheckout(
            commitId: "commit-old",
            resources: [checkoutResource(id: "memory", path: "memory.md", hash: "hash-old")]
        )

        let plan = MemorySyncPlan.staleResourcePlan(
            displayedResources: [displayed],
            projectName: "Project",
            observedProjectRefCommitId: "commit-new",
            authoritativeCommitId: "commit-old",
            serverCursor: "commit-old",
            checkout: checkout,
            authoritativeResponseIsStale: true
        )

        XCTAssertNil(plan)
    }

    func testStaleCacheHeaderIsRecognizedCaseInsensitively() {
        XCTAssertTrue(DaemonServerResponse(
            status: 200,
            headers: ["X-ClUmSiEs-CaChE": "StAlE"],
            body: "{}"
        ).isStaleCache)
        XCTAssertFalse(DaemonServerResponse(
            status: 200,
            headers: ["x-clumsies-cache": "live"],
            body: "{}"
        ).isStaleCache)
    }

    func testOrgAuthorityReconciliationPreservesOnlyProvenLoadedBodies() {
        let unchangedBody = "unchanged body"
        let unchangedHash = contentHash(unchangedBody)
        let updatedBody = "old body"
        let existing = [
            orgResource(
                id: "unchanged",
                path: "unchanged.md",
                body: unchangedBody,
                hash: unchangedHash,
                commitId: "org-commit-old"
            ),
            orgResource(
                id: "updated",
                path: "updated.md",
                body: updatedBody,
                hash: contentHash(updatedBody),
                commitId: "org-commit-old"
            ),
            orgResource(
                id: "deleted",
                path: "deleted.md",
                body: "deleted body",
                hash: contentHash("deleted body"),
                commitId: "org-commit-old"
            ),
        ]
        let authoritative = [
            orgResource(
                id: "unchanged",
                path: "unchanged.md",
                body: "",
                hash: unchangedHash,
                commitId: "org-commit-new",
                contentLoaded: false
            ),
            orgResource(
                id: "updated",
                path: "updated.md",
                body: "",
                hash: contentHash("new body"),
                commitId: "org-commit-new",
                contentLoaded: false
            ),
            orgResource(
                id: "added",
                path: "added.md",
                body: "",
                hash: contentHash("added body"),
                commitId: "org-commit-new",
                contentLoaded: false
            ),
        ]

        let reconciled = MemorySyncPlan.reconciledOrgResources(
            existing: existing,
            authoritative: authoritative
        )
        let byId = Dictionary(uniqueKeysWithValues: reconciled.map { ($0.id, $0) })

        XCTAssertEqual(Set(byId.keys), ["unchanged", "updated", "added"])
        XCTAssertEqual(byId["unchanged"]?.document.body, unchangedBody)
        XCTAssertTrue(byId["unchanged"]?.contentLoaded == true)
        XCTAssertEqual(byId["unchanged"]?.refCommitId, "org-commit-new")
        XCTAssertEqual(byId["updated"]?.document.body, "")
        XCTAssertFalse(byId["updated"]?.contentLoaded == true)
        XCTAssertEqual(byId["updated"]?.contentHash, contentHash("new body"))
        XCTAssertEqual(byId["added"]?.document.body, "")
        XCTAssertFalse(byId["added"]?.contentLoaded == true)
        XCTAssertNil(byId["deleted"])
    }

    func testOrgAuthoritySnapshotRejectsStaleCache() {
        XCTAssertNil(MemoryCatalog.stableOrgAuthorityCommitId(
            beforeCommitId: "org-commit-new",
            afterCommitId: "org-commit-new",
            responseIsStale: true
        ))
    }

    func testOrgAuthoritySnapshotRejectsCommitChangeDuringListing() {
        XCTAssertEqual(MemoryCatalog.stableOrgAuthorityCommitId(
            beforeCommitId: "org-commit-new",
            afterCommitId: "org-commit-new",
            responseIsStale: false
        ), "org-commit-new")
        XCTAssertNil(MemoryCatalog.stableOrgAuthorityCommitId(
            beforeCommitId: "org-commit-before",
            afterCommitId: "org-commit-after",
            responseIsStale: false
        ))
    }

    func testStaleResourcePlanIsEmptyWhenDisplayedRefIsAuthoritative() {
        let displayed = projectResource(
            id: "memory",
            path: "memory.md",
            hash: "hash",
            commitId: "commit-current"
        )
        let checkout = projectCheckout(
            commitId: "commit-current",
            resources: [checkoutResource(id: "memory", path: "memory.md", hash: "other-hash")]
        )

        let plan = MemorySyncPlan.staleResourcePlan(
            displayedResources: [displayed],
            projectName: "Project",
            observedProjectRefCommitId: "commit-current",
            authoritativeCommitId: "commit-current",
            serverCursor: "commit-current",
            checkout: checkout
        )

        XCTAssertEqual(plan, [:])
    }

    func testStaleResourcePlanCapturesProjectAndSelectedOrgChanges() {
        let renamed = projectResource(id: "renamed", path: "old.md", hash: "same-hash")
        let deleted = projectResource(id: "deleted", path: "deleted.md", hash: "deleted-hash")
        let unchanged = projectResource(id: "unchanged", path: "same.md", hash: "same")
        let org = orgResource(
            id: "org-memory",
            path: "org.md",
            body: "old org",
            commitId: "org-commit-old"
        )
        let remoteOrgBody = "remote-org-memory"
        let remoteOrgHash = contentHash(remoteOrgBody)
        let authoritativeOrg = orgResource(
            id: "org-memory",
            path: "org.md",
            body: "",
            hash: remoteOrgHash,
            commitId: "org-commit-new",
            contentLoaded: false
        )
        let checkout = projectCheckout(
            commitId: "commit-new",
            resources: [
                checkoutResource(id: "renamed", path: "new.md", hash: "same-hash"),
                checkoutResource(id: "unchanged", path: "same.md", hash: "same"),
                checkoutResource(id: "added", path: "added.md", hash: "added-hash"),
                checkoutResource(
                    id: "org-memory",
                    path: "org.md",
                    hash: remoteOrgHash,
                    scope: .org,
                    body: remoteOrgBody
                ),
            ]
        )

        let plan = MemorySyncPlan.staleResourcePlan(
            displayedResources: [renamed, deleted, unchanged, org],
            projectName: "Project",
            observedProjectRefCommitId: "commit-old",
            observedSelectedOrgResourceIds: ["org-memory"],
            authoritativeCommitId: "commit-new",
            serverCursor: "commit-new",
            checkout: checkout,
            authoritativeRefEtag: "\"server-commit-new\"",
            authoritativeOrgResources: [authoritativeOrg],
            authoritativeOrgRefCommitId: "org-commit-new",
            generation: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!
        )

        XCTAssertEqual(
            Set(plan?.keys.map { $0 } ?? []),
            ["renamed", "deleted", "added", "org-memory"]
        )
        XCTAssertEqual(plan?["renamed"]?.local?.document.path, "old.md")
        XCTAssertEqual(plan?["renamed"]?.remote?.document.path, "new.md")
        XCTAssertNotNil(plan?["deleted"]?.local)
        XCTAssertNil(plan?["deleted"]?.remote)
        XCTAssertNil(plan?["added"]?.local)
        XCTAssertNotNil(plan?["added"]?.remote)
        XCTAssertEqual(plan?["org-memory"]?.local?.document.body, "old org")
        XCTAssertEqual(plan?["org-memory"]?.remote?.document.body, remoteOrgBody)
        XCTAssertEqual(plan?["org-memory"]?.remote?.refCommitId, "org-commit-new")
        XCTAssertEqual(plan?["renamed"]?.authoritativeRefEtag, "\"server-commit-new\"")
        XCTAssertNil(plan?["unchanged"])
    }

    func testSelectedOrgRenameBuildsAForwardPlan() {
        let body = "same body"
        let hash = contentHash(body)
        let local = orgResource(
            id: "org-memory",
            path: "old.md",
            body: body,
            hash: hash,
            commitId: "org-commit-old"
        )
        let authority = orgResource(
            id: "org-memory",
            path: "renamed.md",
            body: "",
            hash: hash,
            commitId: "org-commit-new",
            contentLoaded: false
        )
        let checkout = projectCheckout(
            commitId: "project-commit-new",
            resources: [checkoutResource(
                id: "org-memory",
                path: "renamed.md",
                hash: hash,
                scope: .org,
                body: body
            )]
        )

        let plan = MemorySyncPlan.staleResourcePlan(
            displayedResources: [local],
            projectName: "Project",
            observedProjectRefCommitId: "project-commit-old",
            observedSelectedOrgResourceIds: ["org-memory"],
            authoritativeCommitId: "project-commit-new",
            serverCursor: "project-commit-new",
            checkout: checkout,
            authoritativeOrgResources: [authority],
            authoritativeOrgRefCommitId: "org-commit-new"
        )

        XCTAssertEqual(plan?["org-memory"]?.local?.document.path, "old.md")
        XCTAssertEqual(plan?["org-memory"]?.remote?.document.path, "renamed.md")
        XCTAssertEqual(plan?["org-memory"]?.remote?.document.body, body)
    }

    func testSelectedOrgDeletionRequiresOrgAuthorityAndDoesNotDeleteADeselection() {
        let local = orgResource(
            id: "org-memory",
            path: "org.md",
            body: "body",
            commitId: "org-commit-old"
        )
        let checkout = projectCheckout(commitId: "project-commit-new", resources: [])

        let deletion = MemorySyncPlan.staleResourcePlan(
            displayedResources: [local],
            projectName: "Project",
            observedProjectRefCommitId: "project-commit-old",
            observedSelectedOrgResourceIds: ["org-memory"],
            authoritativeCommitId: "project-commit-new",
            serverCursor: "project-commit-new",
            checkout: checkout,
            authoritativeOrgResources: [],
            authoritativeOrgRefCommitId: "org-commit-new"
        )
        let deselection = MemorySyncPlan.staleResourcePlan(
            displayedResources: [local],
            projectName: "Project",
            observedProjectRefCommitId: "project-commit-old",
            observedSelectedOrgResourceIds: ["org-memory"],
            authoritativeCommitId: "project-commit-new",
            serverCursor: "project-commit-new",
            checkout: checkout,
            authoritativeOrgResources: [local],
            authoritativeOrgRefCommitId: "org-commit-old"
        )

        XCTAssertNotNil(deletion?["org-memory"]?.local)
        XCTAssertNil(deletion?["org-memory"]?.remote)
        XCTAssertEqual(deselection, [:])
    }

    func testSelectedOrgPlanRejectsStaleOrMismatchedAuthority() {
        let oldBody = "old body"
        let oldHash = contentHash(oldBody)
        let newBody = "new body"
        let newHash = contentHash(newBody)
        let local = orgResource(
            id: "org-memory",
            path: "org.md",
            body: newBody,
            hash: newHash,
            commitId: "org-commit-new"
        )
        let historicalAuthority = orgResource(
            id: "org-memory",
            path: "org.md",
            body: "",
            hash: newHash,
            commitId: "org-commit-new",
            contentLoaded: false
        )
        let checkout = projectCheckout(
            commitId: "project-commit-new",
            resources: [checkoutResource(
                id: "org-memory",
                path: "org.md",
                hash: oldHash,
                scope: .org,
                body: oldBody
            )]
        )
        let currentCheckout = projectCheckout(
            commitId: "project-commit-new",
            resources: [checkoutResource(
                id: "org-memory",
                path: "org.md",
                hash: newHash,
                scope: .org,
                body: newBody
            )]
        )

        XCTAssertNil(MemorySyncPlan.staleResourcePlan(
            displayedResources: [local],
            projectName: "Project",
            observedProjectRefCommitId: "project-commit-old",
            observedSelectedOrgResourceIds: ["org-memory"],
            authoritativeCommitId: "project-commit-new",
            serverCursor: "project-commit-new",
            checkout: checkout,
            authoritativeOrgResources: [historicalAuthority],
            authoritativeOrgRefCommitId: "org-commit-new"
        ))
        XCTAssertNil(MemorySyncPlan.staleResourcePlan(
            displayedResources: [local],
            projectName: "Project",
            observedProjectRefCommitId: "project-commit-old",
            observedSelectedOrgResourceIds: ["org-memory"],
            authoritativeCommitId: "project-commit-new",
            serverCursor: "project-commit-new",
            checkout: currentCheckout,
            authoritativeOrgResources: [historicalAuthority],
            authoritativeOrgRefCommitId: "org-commit-new",
            authoritativeOrgResponseIsStale: true
        ))
    }

    func testSelectedOrgPlanRepairsAnOldBodyMislabeledAsTheCurrentGeneration() {
        let oldBody = "old body"
        let newBody = "new body"
        let newHash = contentHash(newBody)
        let mislabeledLocal = orgResource(
            id: "org-memory",
            path: "org.md",
            body: oldBody,
            hash: newHash,
            commitId: "org-commit-new"
        )
        let authority = orgResource(
            id: "org-memory",
            path: "org.md",
            body: "",
            hash: newHash,
            commitId: "org-commit-new",
            contentLoaded: false
        )
        let checkout = projectCheckout(
            commitId: "project-commit-new",
            resources: [checkoutResource(
                id: "org-memory",
                path: "org.md",
                hash: newHash,
                scope: .org,
                body: newBody
            )]
        )

        let plan = MemorySyncPlan.staleResourcePlan(
            displayedResources: [mislabeledLocal],
            projectName: "Project",
            observedProjectRefCommitId: "project-commit-old",
            observedSelectedOrgResourceIds: ["org-memory"],
            authoritativeCommitId: "project-commit-new",
            serverCursor: "project-commit-new",
            checkout: checkout,
            authoritativeOrgResources: [authority],
            authoritativeOrgRefCommitId: "org-commit-new"
        )

        XCTAssertEqual(plan?["org-memory"]?.local?.document.body, oldBody)
        XCTAssertEqual(plan?["org-memory"]?.remote?.document.body, newBody)
    }

    func testStaleResourcePlanKeepsAProvisionalAdditionPending() {
        let provisional = projectResource(
            id: "added",
            path: "added.md",
            hash: "added-hash",
            commitId: "commit-new"
        )
        let checkout = projectCheckout(
            commitId: "commit-new",
            resources: [checkoutResource(id: "added", path: "added.md", hash: "added-hash")]
        )

        let plan = MemorySyncPlan.staleResourcePlan(
            displayedResources: [provisional],
            projectName: "Project",
            observedProjectRefCommitId: "commit-old",
            authoritativeCommitId: "commit-new",
            serverCursor: "commit-new",
            checkout: checkout,
            provisionalResourceIds: ["added"]
        )

        XCTAssertNil(plan?["added"]?.local)
        XCTAssertNotNil(plan?["added"]?.remote)
    }

    func testEquivalentStalePlansIgnoreDetectionGeneration() {
        let displayed = projectResource(id: "memory", path: "memory.md", hash: "old")
        let checkout = projectCheckout(
            commitId: "commit-new",
            resources: [checkoutResource(id: "memory", path: "memory.md", hash: "new")]
        )
        let first = MemorySyncPlan.staleResourcePlan(
            displayedResources: [displayed],
            projectName: "Project",
            observedProjectRefCommitId: "commit-old",
            authoritativeCommitId: "commit-new",
            serverCursor: "commit-new",
            checkout: checkout,
            generation: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!
        ) ?? [:]
        let second = MemorySyncPlan.staleResourcePlan(
            displayedResources: [displayed],
            projectName: "Project",
            observedProjectRefCommitId: "commit-old",
            authoritativeCommitId: "commit-new",
            serverCursor: "commit-new",
            checkout: checkout,
            generation: UUID(uuidString: "00000000-0000-0000-0000-000000000002")!
        ) ?? [:]

        XCTAssertNotEqual(first["memory"]?.generation, second["memory"]?.generation)
        XCTAssertTrue(MemorySyncPlan.staleResourcePlansMatch(first, second))
    }

    func testReviewContributionToggleIncludesAllEligibleChangesAndPreservesOrgIdentity() async {
        let draft = localDraft(id: "project-draft", targetId: nil)
        var adaptation = localDraft(id: "adaptation", targetId: "project-memory")
        adaptation.orgSource = .init(resourceId: "org-source", commitId: "org-base")
        var deletion = localDraft(id: "deletion", targetId: "deleted-memory")
        deletion.isDeletion = true
        var empty = localDraft(id: "empty", targetId: "unchanged-memory")
        empty.hasChanges = false
        let pending = localDraft(id: "pending", targetId: "pending-memory", status: .submitted)
        var submitted: [OrgContributionEntry] = []
        let model = ReviewRequestModel(initialTitle: "Publish", drafts: [draft, adaptation, deletion, empty, pending],
            loadCandidates: { [] }) { _, _, _, entries in submitted = entries }
        XCTAssertTrue(model.contributionEntries.isEmpty)
        XCTAssertTrue(model.canContribute)
        model.contributesToOrg = true
        let saved = await model.submit()
        XCTAssertTrue(saved)
        XCTAssertEqual(submitted, [
            .init(draftId: draft.id, targetId: nil, path: draft.document.path),
            .init(draftId: adaptation.id, targetId: "org-source", path: nil)
        ])
        model.contributesToOrg = false
        let withoutContribution = await model.submit()
        XCTAssertTrue(withoutContribution)
        XCTAssertTrue(submitted.isEmpty)
        let unavailable = ReviewRequestModel(initialTitle: "Remove", drafts: [deletion, empty, pending],
            loadCandidates: { [] }) { _, _, _, _ in }
        XCTAssertFalse(unavailable.canContribute)
        unavailable.contributesToOrg = true
        XCTAssertTrue(unavailable.contributionEntries.isEmpty)
    }

    func testPublishedUpdatesInstallAutomaticallyWhileDraftsKeepTheirBaseline() {
        let workspace = WorkspaceCoordinator()
        workspace.context.activeProjectId = "project"
        workspace.context.projects = [.init(id: "project", name: "Project", refCommitId: "old", refEtag: "old", selectedOrgResourceIds: [], orgSelectionRevision: 0, isLoaded: true)]
        let local = projectResource(id: "memory", path: "guide.md", hash: "old")
        var remote = projectResource(id: "memory", path: "guide.md", hash: "new", commitId: "new")
        remote.document.body = "Published update"
        let snapshot = StaleResourceSyncSnapshot(projectId: "project", observedProjectRefCommitId: "old", observedSelectedOrgResourceIds: [], observedOrgSelectionRevision: 0, authoritativeCommitId: "new", authoritativeRefEtag: nil, selectedOrgResourceIds: [], orgSelectionRevision: 0, generation: UUID(), local: local, remote: remote)
        workspace.catalog.resources = [local]
        workspace.catalog.installStaleResourcePlan([local.id: snapshot], for: "project")
        workspace.edits.drafts = [localDraft(id: "my-draft", targetId: local.id)]
        workspace.sync.applyUneditedUpdates(projectId: "project")
        XCTAssertEqual(workspace.catalog.resources.first?.contentHash, "old")
        XCTAssertNotNil(workspace.catalog.staleResourceSnapshots[local.id])
        workspace.edits.drafts = []
        workspace.sync.applyUneditedUpdates(projectId: "project")
        XCTAssertEqual(workspace.catalog.resources.first?.document.body, "Published update")
        XCTAssertTrue(workspace.catalog.staleResourceSnapshots.isEmpty)
        XCTAssertEqual(workspace.context.projects.first?.refCommitId, "new")
    }

    func testDocumentPathChangesAttributeRemoteRenameAndDeletionToShared() {
        XCTAssertEqual(
            MemoryModel.documentPathChanges(
                basePath: "old.md",
                localPath: "old.md",
                remotePath: "new.md"
            ),
            [.init(source: .shared, from: "old.md", to: "new.md")]
        )
        XCTAssertEqual(
            MemoryModel.documentPathChanges(
                basePath: "old.md",
                localPath: "old.md",
                remotePath: nil
            ),
            [.init(source: .shared, from: "old.md", to: nil)]
        )
    }

    func testDocumentPathChangesKeepDivergentDraftAndSharedRenamesSeparate() {
        XCTAssertEqual(
            MemoryModel.documentPathChanges(
                basePath: "base.md",
                localPath: "draft.md",
                remotePath: "shared.md"
            ),
            [
                .init(source: .draft, from: "base.md", to: "draft.md"),
                .init(source: .shared, from: "base.md", to: "shared.md"),
            ]
        )
    }

    func testMemoryContentValidationRejectsStaleAndMismatchedBodies() throws {
        let hash = "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        let resource = projectResource(
            id: "memory",
            path: "memory.md",
            hash: hash,
            commitId: "commit-current"
        )
        let metadata = MemoryMetadata(
            memoryId: "memory",
            scope: "project",
            projectId: "project",
            path: "memory.md",
            name: "Memory",
            description: "",
            contentHash: hash,
            status: "active",
            updatedAt: "2026-08-19T00:00:00Z"
        )
        let live = DaemonServerResponse(status: 200, headers: [:], body: "{}")

        XCTAssertEqual(
            try WorkspaceLoader.validatedMemoryContent(
                for: resource,
                detail: .init(memory: metadata, content: "hello", etag: "etag"),
                response: live
            ),
            "hello"
        )
        XCTAssertThrowsError(try WorkspaceLoader.validatedMemoryContent(
            for: resource,
            detail: .init(memory: metadata, content: "wrong body", etag: "etag"),
            response: live
        ))
        XCTAssertThrowsError(try WorkspaceLoader.validatedMemoryContent(
            for: resource,
            detail: .init(memory: metadata, content: "hello", etag: "etag"),
            response: .init(
                status: 200,
                headers: ["x-clumsies-cache": "stale"],
                body: "{}"
            )
        ))
    }

    func testResourceGenerationComparisonRejectsCommitAndHashChanges() {
        let old = projectResource(
            id: "memory",
            path: "memory.md",
            hash: "old",
            commitId: "commit-old"
        )
        let current = projectResource(
            id: "memory",
            path: "memory.md",
            hash: "new",
            commitId: "commit-new"
        )

        XCTAssertFalse(MemoryCatalog.resourceGenerationMatches(old, current))
        XCTAssertTrue(MemoryCatalog.resourceGenerationMatches(current, current))
    }

    func testCancelledDeferredMemoryLoadDoesNotPresentError() async {
        let store = WorkspaceCoordinator()
        let resource = orgResource(id: "memory", contentLoaded: false)
        let item = MemoryListItem(
            id: resource.id,
            resource: resource,
            draft: nil,
            inherited: false
        )

        await Task { @MainActor in
            withUnsafeCurrentTask { $0?.cancel() }
            await store.catalog.loadContentIfNeeded(item)
        }.value

        XCTAssertNil(store.feedback.errorMessage)
        XCTAssertFalse(store.catalog.loadingResourceIds.contains(resource.id))
    }

    func testFailedMemoryLoadReturnsAnInlineFailureAndCanRetry() async {
        let store = WorkspaceCoordinator()
        let resource = orgResource(id: "memory", contentLoaded: false)
        let item = MemoryListItem(id: resource.id, resource: resource, draft: nil, inherited: false)
        let failed = await store.catalog.loadContentIfNeeded(item) { _ in throw URLError(.notConnectedToInternet) }
        XCTAssertNotNil(failed)
        XCTAssertNil(store.feedback.errorMessage)
        XCTAssertTrue(store.catalog.loadingResourceIds.isEmpty)

        let retried = await store.catalog.loadContentIfNeeded(item) { resource in
            var loaded = resource
            loaded.contentLoaded = true
            loaded.document.body = "Loaded after retry"
            return loaded
        }
        XCTAssertNil(retried)
        XCTAssertTrue(store.catalog.loadingResourceIds.isEmpty)
    }

    func testConcurrentMemoryReadersShareTheRequestAndItsFailure() async {
        let store = WorkspaceCoordinator()
        let resource = orgResource(id: "memory", contentLoaded: false)
        let item = MemoryListItem(id: resource.id, resource: resource, draft: nil, inherited: false)
        let started = expectation(description: "Content request started")
        let secondEntered = expectation(description: "Second reader joined")
        let release = WorkspaceNavigationTestLatch()
        let first = Task {
            await store.catalog.loadContentIfNeeded(item) { _ in
                started.fulfill()
                await release.wait()
                throw URLError(.timedOut)
            }
        }
        await fulfillment(of: [started], timeout: 1)
        let second = Task {
            secondEntered.fulfill()
            return await store.catalog.loadContentIfNeeded(item) { resource in
                XCTFail("A concurrent reader must not start another content request")
                return resource
            }
        }
        await fulfillment(of: [secondEntered], timeout: 1)
        await Task.yield()
        await release.open()
        let firstFailure = await first.value
        let secondFailure = await second.value
        XCTAssertNotNil(firstFailure)
        XCTAssertEqual(firstFailure, secondFailure)
        XCTAssertTrue(store.catalog.loadingResourceIds.isEmpty)
    }

    func testRenameOnlyDraftDoesNotTreatAnUnloadedOrphanBaselineAsEditableContent() {
        var unloaded = projectResource(
            id: "removed-memory",
            path: "old.md",
            hash: "sha256:old",
            commitId: "commit-old"
        )
        unloaded.contentLoaded = false
        unloaded.document.body = ""
        let summary = DaemonDraftSummary(
            draftId: "draft",
            projectId: "project",
            serverDraftId: "server-draft",
            serverVersion: 1,
            baseCommitId: "commit-old",
            currentCommitId: "commit-new",
            freshness: .behind,
            hasUpstreamResourceChanges: true,
            reconciliation: .unknown,
            reconciliationCandidateId: nil,
            scope: .project,
            resourceKind: .memory,
            targetId: "removed-memory",
            path: "old.md",
            status: .open,
            createdAt: "2026-08-19T00:00:00Z",
            updatedAt: "2026-08-19T00:00:00Z",
            pendingOperationCount: 0,
            failedOperationCount: 0
        )
        let operation = DaemonLocalDraftOperation(
            localOperationId: "operation",
            resourceKind: .memory,
            operation: .rename(
                id: "removed-memory",
                newPath: "renamed.md",
                description: nil
            ),
            source: .desktop,
            syncStatus: .synced,
            lastError: nil,
            createdAt: "2026-08-19T00:00:00Z",
            updatedAt: "2026-08-19T00:00:00Z"
        )

        let mapped = WorkspaceLoader.mapDraft(
            .init(draft: summary, operations: [operation]),
            resources: [unloaded]
        )

        XCTAssertFalse(mapped.documentBaselineAvailable)
        XCTAssertEqual(mapped.document.path, "renamed.md")
    }

    func testUnloadedResourceRenamePlanCarriesNoPlaceholderContent() {
        var resource = projectResource(
            id: "memory",
            path: "old.md",
            hash: "sha256:body"
        )
        resource.contentLoaded = false
        resource.document.body = ""
        let item = MemoryListItem(
            id: resource.id,
            resource: resource,
            draft: nil,
            inherited: false
        )

        let plan = DraftStore.documentRenamePlan(
            for: item,
            currentDraft: nil,
            newPath: "renamed.md"
        )

        XCTAssertEqual(
            plan,
            .init(targetId: "memory", newPath: "renamed.md")
        )
        XCTAssertTrue(MemoryFileTreeMenu.canRename(item, inOrgView: false))

        let dirty = EditableMemoryDocument(
            title: "old.md",
            path: "old.md",
            body: "unsaved body"
        )
        let retargeted = DraftStore.documentByRetargetingPendingSave(
            dirty,
            to: "renamed.md"
        )
        XCTAssertEqual(retargeted.path, "renamed.md")
        XCTAssertEqual(retargeted.body, "unsaved body")
    }

    func testPureCreateDraftRenameUsesItsProvisionalDraftId() throws {
        let draft = localDraft(id: "draft", targetId: nil, scope: .org)
        let item = MemoryListItem(
            id: draft.id,
            resource: nil,
            draft: draft,
            inherited: false
        )

        let plan = try XCTUnwrap(DraftStore.documentRenamePlan(
            for: item,
            currentDraft: draft,
            newPath: "renamed.md"
        ))
        XCTAssertEqual(plan.targetId, draft.id)
        XCTAssertEqual(plan.newPath, "renamed.md")
        XCTAssertTrue(MemoryFileTreeMenu.canRename(item, inOrgView: false))
    }

    func testDraftUploadBarrierRequiresASettledServerDraft() {
        XCTAssertEqual(
            DraftReconciliationService.draftUploadBarrierDecision(
                serverDraftId: nil,
                pendingOperationCount: 1,
                failedOperationCount: 0,
                operationStates: [.queued],
                failureMessage: nil
            ),
            .wait
        )
        XCTAssertEqual(
            DraftReconciliationService.draftUploadBarrierDecision(
                serverDraftId: "server-draft",
                pendingOperationCount: 0,
                failedOperationCount: 0,
                operationStates: [.synced],
                failureMessage: nil
            ),
            .ready
        )
        XCTAssertEqual(
            DraftReconciliationService.draftUploadBarrierDecision(
                serverDraftId: "server-draft",
                pendingOperationCount: 0,
                failedOperationCount: 1,
                operationStates: [.failed],
                failureMessage: "upload failed"
            ),
            .failed("upload failed")
        )
    }

    func testStaleDiffRefusesAnUnloadedHistoricalBaseline() {
        var local = projectResource(id: "memory", path: "memory.md", hash: "old")
        local.contentLoaded = false
        local.document.body = ""
        let remote = projectResource(
            id: "memory",
            path: "memory.md",
            hash: "new",
            commitId: "commit-new"
        )
        let snapshot = StaleResourceSyncSnapshot(
            projectId: "project",
            observedProjectRefCommitId: "commit-old",
            observedSelectedOrgResourceIds: [],
            observedOrgSelectionRevision: 1,
            authoritativeCommitId: "commit-new",
            authoritativeRefEtag: "\"commit-new\"",
            selectedOrgResourceIds: [],
            orgSelectionRevision: 1,
            generation: UUID(),
            local: local,
            remote: remote
        )

        XCTAssertThrowsError(try MemoryModel.staleDocumentDiffTexts(snapshot)) { error in
            XCTAssertEqual(error as? DocumentDiffError, .baselineUnavailable)
        }
    }

    func testUnrepresentedDraftsKeepsTargetBackedDraftWhenAuthoritativeTargetIsMissing() {
        let missingTarget = localDraft(
            id: "draft-for-removed-resource",
            targetId: "removed-resource"
        )

        let unrepresented = MemoryTreeProjection.unrepresentedDrafts(
            [missingTarget],
            authoritativeResourceIds: []
        )

        XCTAssertEqual(unrepresented.map(\.id), [missingTarget.id])
    }

    func testUnrepresentedDraftsFiltersDraftWithAnAuthoritativeTarget() {
        let represented = localDraft(
            id: "draft-for-current-resource",
            targetId: "current-resource"
        )

        let unrepresented = MemoryTreeProjection.unrepresentedDrafts(
            [represented],
            authoritativeResourceIds: ["current-resource"]
        )

        XCTAssertTrue(unrepresented.isEmpty)
    }

    func testMissingTargetDraftUsesTargetIdAsItsStableItemIdentity() {
        let missingTarget = localDraft(
            id: "local-draft-id",
            targetId: "removed-authoritative-resource"
        )
        let unrepresented = MemoryTreeProjection.unrepresentedDrafts(
            [missingTarget],
            authoritativeResourceIds: []
        )

        let itemIds = unrepresented.map { $0.targetId ?? $0.id }

        XCTAssertEqual(itemIds, ["removed-authoritative-resource"])
        XCTAssertNotEqual(itemIds.first, missingTarget.id)
    }

    func testCenteredTextViewUsesMinimumInsetInNarrowPane() {
        XCTAssertEqual(
            CenteredTextView.horizontalInset(for: 700),
            DocumentContentMetrics.minimumHorizontalInset
        )
    }

    func testCenteredTextViewCentersReadableColumnInWidePane() {
        let paneWidth: CGFloat = 1_200

        XCTAssertEqual(
            CenteredTextView.horizontalInset(for: paneWidth),
            (paneWidth - DocumentContentMetrics.maximumWidth) / 2
        )
    }

    func testDocumentReconciliationOwnsAWindowAndSurvivesClosingTheSourceTab() async throws {
        let workspace = WorkspaceCoordinator()
        let windows = ReconciliationWindows(store: workspace)
        workspace.context.activeProjectId = "project"
        let draft = localDraft(id: "draft", targetId: "memory")
        let item = MemoryListItem(id: "memory", resource: nil, draft: draft,
                                  inherited: false, projectContextId: "project")
        let key = try XCTUnwrap(workspace.sessions.documentSessionKey(for: item))
        let resource = ServerDraftResourceReference(scope: "project", id: "memory", path: "memory.md")
        let state = ReconciliationResourceState(exists: true, resource: resource,
            content: .init(description: nil, content: draft.document.body))
        let candidate = DraftReconciliationCandidate(candidateId: "candidate", draftId: "server-draft",
            draftVersion: 1, baseCommitId: "base", currentCommitId: "remote", status: .conflicts,
            baseState: state, currentState: state, draftState: state, proposedState: nil,
            conflicts: [.init(kind: "modify_modify", field: "content", base: "base",
                              current: "remote", draft: "draft")],
            resultHash: "result", valid: true, createdAt: draft.updatedAt, invalidatedAt: nil)
        let model = DocumentEditorModel(item: item, drafts: workspace.edits, context: workspace.context,
            feedback: workspace.feedback, sessions: workspace.sessions, memory: workspace.memory,
            reviews: workspace.reviews, reconciliation: workspace.reconciliation)
        let host = NSHostingView(rootView: DocumentSessionView(item: item, mode: .source, model: model)
            .workspaceEnvironment(workspace))
        host.sizingOptions = []
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 820),
            styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer {
            workspace.sessions.finishDocumentReconciliation(for: key)
            windows.documentWindows[key]?.dismiss()
            window.close()
        }
        func editors(_ view: NSView) -> [NSTextView] {
            (view as? NSTextView).map { [$0] } ?? view.subviews.flatMap(editors)
        }
        host.layoutSubtreeIfNeeded()
        let originalEditor = try XCTUnwrap(editors(host).first)
        let originalFrame = window.frame
        workspace.sessions.synchronizingDocumentSessions.insert(key)
        workspace.sessions.pendingDocumentReconciliationCandidatesBySession[key] = candidate
        for _ in 0..<20 {
            host.layoutSubtreeIfNeeded()
            if windows.documentWindows[key] != nil { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        let controller = try XCTUnwrap(windows.documentWindows[key])
        let mergeWindow = try XCTUnwrap(controller.window)
        XCTAssertNil(window.attachedSheet)
        XCTAssertNil(mergeWindow.sheetParent)
        XCTAssertTrue(mergeWindow.styleMask.contains([.titled, .closable, .miniaturizable, .resizable]))
        for button in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            XCTAssertFalse(try XCTUnwrap(mergeWindow.standardWindowButton(button)).isHidden)
        }
        let content = try XCTUnwrap(mergeWindow.contentView)
        for _ in 0..<3 {
            content.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertTrue(editors(host).first === originalEditor, "the document must stay mounted")
        XCTAssertEqual(originalEditor.string, draft.document.body)
        XCTAssertEqual(window.frame.size, originalFrame.size)
        XCTAssertTrue(editors(content).isEmpty, "document reconciliation uses the same choices and diff as Review")
        let tab = tab(itemId: item.id, projectId: "project")
        workspace.navigation.tabs = [tab]
        workspace.navigation.closeTab(tab)
        XCTAssertTrue(workspace.navigation.tabs.isEmpty)
        XCTAssertNotNil(workspace.sessions.pendingDocumentReconciliationCandidatesBySession[key])
        window.close()
        XCTAssertTrue(mergeWindow.isVisible, "the merge must outlive its source window")

        var resolution = DraftResolution(candidate: candidate)
        resolution.chooseFile(candidate.draftState)
        XCTAssertTrue(resolution.canSave)
        workspace.sessions.documentReconciliationResolutions[key] = resolution
        controller.confirmDiscard = { false }
        mergeWindow.performClose(nil)
        XCTAssertTrue(mergeWindow.isVisible)
        XCTAssertEqual(workspace.sessions.documentReconciliationResolutions[key], resolution)
        controller.confirmDiscard = { true }
        workspace.sessions.applyingDocumentReconciliationSessions.insert(key)
        XCTAssertFalse(controller.windowShouldClose(mergeWindow), "saving cannot be interrupted by closing")
        workspace.sessions.applyingDocumentReconciliationSessions.remove(key)
        mergeWindow.performClose(nil)
        for _ in 0..<20 {
            if windows.documentWindows[key] == nil { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertNil(windows.documentWindows[key])
        XCTAssertFalse(workspace.sessions.isSynchronizingDocument(item.id))
        XCTAssertTrue(workspace.sessions.canCommitMemoryContextSwitch)
    }

    func testKeepFileAfterDeleteConflictUsesCurrentContentTemplate() {
        let resource = ServerDraftResourceReference(
            scope: "project",
            id: "memory",
            path: "memory.md"
        )
        let deleted = ReconciliationResourceState(
            exists: false,
            resource: resource,
            content: nil
        )
        let candidate = DraftReconciliationCandidate(
            candidateId: "candidate",
            draftId: "draft",
            draftVersion: 2,
            baseCommitId: "base",
            currentCommitId: "current",
            status: .conflicts,
            baseState: .init(
                exists: true,
                resource: resource,
                content: .init(description: "base description", content: "Base body")
            ),
            currentState: .init(
                exists: true,
                resource: resource,
                content: .init(description: "current description", content: "Remote body")
            ),
            draftState: deleted,
            proposedState: deleted,
            conflicts: [
                .init(kind: "delete_modify", field: "exists", base: "true", current: "true", draft: "false")
            ],
            resultHash: nil,
            valid: true,
            createdAt: "2026-08-19T00:00:00Z",
            invalidatedAt: nil
        )

        var resolution = DraftResolution(candidate: candidate)
        XCTAssertFalse(resolution.canSave)
        resolution.chooseFile(candidate.currentState)
        XCTAssertTrue(resolution.canSave)
        XCTAssertEqual(resolution.state.content?.primaryText, "Remote body")
        XCTAssertEqual(resolution.state.content?.description, "current description")
    }

    func testOrgViewPresentationStripsProjectCarriedDraft() {
        let item = MemoryListItem(
            id: "org-resource",
            resource: MemoryResource(
                id: "org-resource",
                scope: .org,
                projectId: nil,
                projectName: nil,
                kind: .context,
                contentHash: "hash",
                updatedAt: "2026-08-05T00:00:00Z",
                refCommitId: "commit",
                contentLoaded: true,
                document: .init(title: "org.md", path: "org.md", body: "")
            ),
            draft: LocalDraft(
                id: "org-draft",
                projectId: "carrying-project",
                serverId: nil,
                serverVersion: 0,
                baseCommitId: "commit",
                currentCommitId: "commit",
                freshness: .current,
                hasUpstreamResourceChanges: false,
                reconciliation: .unknown,
                reconciliationCandidateId: nil,
                scope: .org,
                kind: .context,
                targetId: "org-resource",
                status: .open,
                origin: .desktop,
                syncStatus: .synced,
                updatedAt: "2026-08-05T00:00:00Z",
                document: .init(title: "org.md", path: "org.md", body: "body"),
                isDeletion: false
            ),
            inherited: false
        )

        let presented = WorkspaceNavigation.memoryItemForViewContext(
            item,
            activeProjectId: nil
        )

        XCTAssertEqual(presented?.resource?.id, "org-resource")
        XCTAssertNil(presented?.draft)
        XCTAssertNil(presented?.projectContextId)
    }

    func testMemoryTreeResourcesUseOrgCatalogInOrgView() {
        let selectedOrg = orgResource(id: "selected-org")
        let unselectedOrg = orgResource(id: "unselected-org")
        let project = projectResource(id: "project", path: "project.md", hash: "project-hash")

        let visible = MemoryTreeProjection.memoryTreeResources(
            [selectedOrg, unselectedOrg, project],
            activeProjectId: nil,
            selectedOrgResourceIds: []
        )

        XCTAssertEqual(Set(visible.map(\.id)), ["selected-org", "unselected-org"])
    }

    func testMemoryTreeResourcesUseSelectedOrgAndCompatibilityProjectAuthority() {
        let selectedOrg = orgResource(id: "selected-org")
        let unselectedOrg = orgResource(id: "unselected-org")
        let project = projectResource(id: "project", path: "project.md", hash: "project-hash")
        let otherProject = projectResource(
            id: "other-project",
            path: "other.md",
            hash: "other-hash",
            projectId: "other"
        )

        let visible = MemoryTreeProjection.memoryTreeResources(
            [selectedOrg, unselectedOrg, project, otherProject],
            activeProjectId: "project",
            selectedOrgResourceIds: ["selected-org"]
        )

        XCTAssertEqual(Set(visible.map(\.id)), ["selected-org", "project"])
    }

    func testMemoryTreeDraftsAreIsolatedByCarryingProject() {
        let current = localDraft(id: "current", targetId: nil)
        let other = localDraft(id: "other", targetId: nil, projectId: "other")
        let org = localDraft(id: "org", targetId: nil, scope: .org)

        XCTAssertEqual(
            MemoryTreeProjection.memoryTreeDrafts(
                [current, other, org],
                activeProjectId: "project"
            ).map(\.id),
            ["current", "org"]
        )
        XCTAssertEqual(
            MemoryTreeProjection.memoryTreeDrafts(
                [current, other, org],
                activeProjectId: nil
            ).map(\.id),
            []
        )
    }

    func testMemoryTabDraftNeverUsesAnotherProjectOrAnOrgTab() {
        let projectP = localDraft(
            id: "draft-p",
            targetId: "memory",
            projectId: "project-p",
            scope: .org,
            updatedAt: "2026-08-19T00:00:00Z"
        )
        let newerProjectQ = localDraft(
            id: "draft-q",
            targetId: "memory",
            projectId: "project-q",
            scope: .org,
            updatedAt: "2026-08-20T00:00:00Z"
        )

        XCTAssertEqual(MemoryTreeProjection.memoryTabDraft(
            itemId: "memory",
            projectId: "project-p",
            drafts: [projectP, newerProjectQ]
        )?.id, "draft-p")
        XCTAssertEqual(MemoryTreeProjection.memoryTabDraft(
            itemId: "memory",
            projectId: "project-q",
            drafts: [projectP, newerProjectQ]
        )?.id, "draft-q")
        XCTAssertNil(MemoryTreeProjection.memoryTabDraft(
            itemId: "memory",
            projectId: nil,
            drafts: [projectP, newerProjectQ]
        ))
    }

    func testMemoryTreePrefersTheNewestDraftForOneTarget() {
        let older = localDraft(
            id: "older",
            targetId: "memory",
            updatedAt: "2026-08-19T00:00:00Z"
        )
        let newer = localDraft(
            id: "newer",
            targetId: "memory",
            updatedAt: "2026-08-20T00:00:00Z"
        )

        XCTAssertEqual(
            MemoryTreeProjection.preferredMemoryTreeDrafts([newer, older]).map(\.id),
            ["newer"]
        )
    }

    func testSelectedOrgItemRetainsItsProjectDraftContext() {
        let resource = orgResource(id: "org")
        let item = MemoryListItem(
            id: resource.id,
            resource: resource,
            draft: nil,
            inherited: true,
            projectContextId: "project"
        )

        XCTAssertEqual(item.projectId, "project")
    }

    func testSameOrgTargetHasDistinctDocumentSessionsPerProject() throws {
        let resource = orgResource(id: "org")
        let projectP = MemoryListItem(
            id: resource.id,
            resource: resource,
            draft: nil,
            inherited: true,
            projectContextId: "project-p"
        )
        let projectQ = MemoryListItem(
            id: resource.id,
            resource: resource,
            draft: nil,
            inherited: true,
            projectContextId: "project-q"
        )

        let keyP = try XCTUnwrap(DocumentSessions.memoryDocumentSessionKey(for: projectP))
        let keyQ = try XCTUnwrap(DocumentSessions.memoryDocumentSessionKey(for: projectQ))
        XCTAssertNotEqual(keyP, keyQ)
        XCTAssertEqual(keyP.itemId, keyQ.itemId)
        XCTAssertEqual(keyP.projectId, "project-p")
        XCTAssertEqual(keyQ.projectId, "project-q")
    }

    func testBothOwnersCanRequestIndependentReviews() {
        let localCreate = localDraft(id: "local", targetId: nil)
        let legacyProjectUpdate = localDraft(id: "legacy", targetId: "project-memory")
        let orgCreate = localDraft(id: "org", targetId: nil, scope: .org)

        XCTAssertTrue(ReviewsModel.canRequestReview(localCreate))
        XCTAssertTrue(ReviewsModel.canRequestReview(legacyProjectUpdate))
        XCTAssertTrue(ReviewsModel.canRequestReview(orgCreate))
    }

    func testProjectReviewExcludesIndependentOrganizationDrafts() {
        let older = localDraft(
            id: "older",
            targetId: "shared",
            scope: .org,
            updatedAt: "2026-08-18T00:00:00Z"
        )
        let newer = localDraft(
            id: "newer",
            targetId: "shared",
            scope: .org,
            updatedAt: "2026-08-20T00:00:00Z"
        )
        let created = localDraft(
            id: "created",
            targetId: nil,
            scope: .org,
            updatedAt: "2026-08-19T00:00:00Z"
        )
        let otherProject = localDraft(
            id: "other",
            targetId: nil,
            projectId: "other-project",
            scope: .org
        )

        XCTAssertEqual(
            ReviewsModel.reviewableProjectDrafts(
                [older, newer, created, otherProject],
                projectId: "project"
            ).map(\.id),
            []
        )

        let legacy = localDraft(id: "legacy", targetId: "legacy-memory")
        XCTAssertEqual(
            ReviewsModel.reviewableProjectDrafts(
                [newer, created, legacy],
                projectId: "project"
            ).map(\.id),
            ["legacy"]
        )
    }

    func testMemoryTabsAreScopedToTheProjectViewContext() {
        let orgTab = tab(itemId: "shared", projectId: nil)
        let projectTab = tab(itemId: "shared", projectId: "project")

        XCTAssertTrue(orgTab.isVisible(in: .memory, projectId: nil))
        XCTAssertFalse(orgTab.isVisible(in: .memory, projectId: "project"))
        XCTAssertTrue(projectTab.isVisible(in: .memory, projectId: "project"))
        XCTAssertFalse(projectTab.isVisible(in: .memory, projectId: nil))
    }

    func testProjectMemoryTabRequiresSelectionOrALocalDraft() {
        let org = orgResource(id: "org")

        XCTAssertFalse(WorkspaceNavigation.memoryTabIsAvailable(
            itemId: org.id,
            projectId: "project",
            selectedOrgResourceIds: [],
            resources: [org],
            drafts: []
        ))
        XCTAssertTrue(WorkspaceNavigation.memoryTabIsAvailable(
            itemId: org.id,
            projectId: "project",
            selectedOrgResourceIds: [org.id],
            resources: [org],
            drafts: []
        ))
        XCTAssertTrue(WorkspaceNavigation.memoryTabIsAvailable(
            itemId: org.id,
            projectId: "project",
            selectedOrgResourceIds: [],
            resources: [org],
            drafts: [localDraft(id: "draft", targetId: org.id, scope: .org)]
        ))

        XCTAssertFalse(WorkspaceNavigation.memoryTabIsAvailable(
            itemId: "discarded-create",
            projectId: "project",
            selectedOrgResourceIds: [],
            resources: [],
            drafts: []
        ))
        XCTAssertTrue(WorkspaceNavigation.memoryTabIsAvailable(
            itemId: "still-loading",
            projectId: "project",
            selectedOrgResourceIds: [],
            resources: [],
            drafts: [],
            allowsUnresolved: true
        ))
    }

    func testOrgMemoryTabClosesAfterItsLocalCreateDisappears() {
        XCTAssertFalse(WorkspaceNavigation.orgMemoryTabIsAvailable(
            itemId: "discarded-create",
            resources: []
        ))
        XCTAssertTrue(WorkspaceNavigation.orgMemoryTabIsAvailable(
            itemId: "org-resource",
            resources: [orgResource(id: "org-resource")]
        ))
        XCTAssertFalse(WorkspaceNavigation.orgMemoryTabIsAvailable(
            itemId: "org-draft",
            resources: []
        ))
    }

    func testDocumentSynchronizationAdmissionIsProjectContextScoped() {
        XCTAssertTrue(DocumentSessions.canStartDocumentSynchronization(
            isSwitchingMemoryContext: false,
            activeProjectId: "project-p",
            itemProjectContextId: "project-p"
        ))
        XCTAssertFalse(DocumentSessions.canStartDocumentSynchronization(
            isSwitchingMemoryContext: true,
            activeProjectId: "project-p",
            itemProjectContextId: "project-p"
        ))
        XCTAssertFalse(DocumentSessions.canStartDocumentSynchronization(
            isSwitchingMemoryContext: false,
            activeProjectId: "project-q",
            itemProjectContextId: "project-p"
        ))
        XCTAssertFalse(DocumentSessions.canStartDocumentSynchronization(
            isSwitchingMemoryContext: false,
            activeProjectId: nil,
            itemProjectContextId: nil
        ))
    }

    func testCapturedProjectOperationStopsAfterContextChanges() {
        XCTAssertTrue(WorkspaceContext.projectContextIsCurrent(
            isSwitchingMemoryContext: false,
            activeProjectId: "project-p",
            expectedProjectId: "project-p"
        ))
        XCTAssertFalse(WorkspaceContext.projectContextIsCurrent(
            isSwitchingMemoryContext: true,
            activeProjectId: "project-p",
            expectedProjectId: "project-p"
        ))
        XCTAssertFalse(WorkspaceContext.projectContextIsCurrent(
            isSwitchingMemoryContext: false,
            activeProjectId: "project-q",
            expectedProjectId: "project-p"
        ))
    }

    func testContextSwitchCommitWaitsForEveryReconciliationActivity() {
        XCTAssertTrue(DocumentSessions.canCommitMemoryContextSwitch(
            hasDocumentSynchronization: false,
            hasApplyingDocumentReconciliation: false,
            hasStandaloneReconciliationActivity: false
        ))
        XCTAssertFalse(DocumentSessions.canCommitMemoryContextSwitch(
            hasDocumentSynchronization: true,
            hasApplyingDocumentReconciliation: false,
            hasStandaloneReconciliationActivity: false
        ))
        XCTAssertFalse(DocumentSessions.canCommitMemoryContextSwitch(
            hasDocumentSynchronization: false,
            hasApplyingDocumentReconciliation: false,
            hasStandaloneReconciliationActivity: true
        ))
    }

    func testRemovingCleanProjectMemoryPrunesItsTabButDraftRetainsIt() {
        let resource = orgResource(id: "org")
        let tab = tab(itemId: resource.id, projectId: "project-p")
        let projectWithoutSelection = ProjectState(
            id: "project-p",
            name: "Project P",
            refCommitId: "commit",
            refEtag: "etag",
            selectedOrgResourceIds: [],
            orgSelectionRevision: 2,
            isLoaded: true
        )
        let projectWithSelection = ProjectState(
            id: "project-p",
            name: "Project P",
            refCommitId: "commit",
            refEtag: "etag",
            selectedOrgResourceIds: [resource.id],
            orgSelectionRevision: 3,
            isLoaded: true
        )

        let afterRemoval = WorkspaceNavigation.retainedMemoryTabs(
            [tab],
            projects: [projectWithoutSelection],
            resources: [resource],
            drafts: []
        )
        XCTAssertTrue(afterRemoval.isEmpty)
        XCTAssertTrue(WorkspaceNavigation.retainedMemoryTabs(
            afterRemoval,
            projects: [projectWithSelection],
            resources: [resource],
            drafts: []
        ).isEmpty)
        XCTAssertEqual(WorkspaceNavigation.retainedMemoryTabs(
            [tab],
            projects: [projectWithoutSelection],
            resources: [resource],
            drafts: [localDraft(
                id: "draft-p",
                targetId: resource.id,
                projectId: "project-p",
                scope: .org
            )]
        ), [tab])
        XCTAssertTrue(WorkspaceNavigation.retainedMemoryTabs(
            [tab],
            projects: [projectWithoutSelection],
            resources: [resource],
            drafts: [localDraft(
                id: "draft-q",
                targetId: resource.id,
                projectId: "project-q",
                scope: .org
            )]
        ).isEmpty)
    }

    func testNewContextDraftStartsWithValidNonemptyContent() {
        XCTAssertEqual(
            MemoryModel.defaultDocument(kind: .context, path: "context/untitled.md").body,
            "# Untitled\n"
        )
    }

    func testNewDraftPathSkipsFreshOrganizationAndLocalDraftCollisions() {
        XCTAssertEqual(
            MemoryModel.uniqueDefaultPath(
                base: "untitled.md",
                occupiedPaths: ["untitled.md", "untitled-2.md"]
            ),
            "untitled-3.md"
        )
        XCTAssertEqual(
            MemoryModel.uniqueDefaultPath(
                base: "workflow/untitled.md",
                occupiedPaths: []
            ),
            "workflow/untitled.md"
        )
    }

    func testProjectSelectionRemovalIsBlockedByItsActiveTargetDraft() {
        let resourceIds: Set<String> = ["org-memory"]
        XCTAssertTrue(MemoryTreeProjection.hasActiveDraft(
            in: "project-p",
            targetingAny: resourceIds,
            drafts: [localDraft(
                id: "draft-p",
                targetId: "org-memory",
                projectId: "project-p",
                scope: .org
            )]
        ))
        XCTAssertFalse(MemoryTreeProjection.hasActiveDraft(
            in: "project-p",
            targetingAny: resourceIds,
            drafts: [localDraft(
                id: "draft-q",
                targetId: "org-memory",
                projectId: "project-q",
                scope: .org
            )]
        ))
    }

    private func tab(
        itemId: String,
        section: WorkspaceSection = .memory,
        projectId: String? = nil
    ) -> WorkbenchTab {
        WorkbenchTab(
            section: section,
            projectId: projectId,
            itemId: itemId,
            mode: .source,
            title: "\(itemId).md"
        )
    }

    private func item(
        path: String,
        body: String = "",
        kind: MemoryKind = .context
    ) -> MemoryListItem {
        let resource = MemoryResource(
            id: path,
            scope: .org,
            projectId: nil,
            projectName: nil,
            kind: kind,
            contentHash: "hash",
            updatedAt: "2026-08-05T00:00:00Z",
            refCommitId: nil,
            contentLoaded: true,
            document: .init(
                title: URL(fileURLWithPath: path).lastPathComponent,
                path: path,
                body: body
            )
        )
        return MemoryListItem(id: resource.id, resource: resource, draft: nil, inherited: false)
    }

    private func projectResource(
        id: String,
        path: String,
        hash: String,
        commitId: String = "commit-old",
        projectId: String = "project"
    ) -> MemoryResource {
        MemoryResource(
            id: id,
            scope: .project,
            projectId: projectId,
            projectName: "Project",
            kind: .context,
            contentHash: hash,
            updatedAt: "2026-08-19T00:00:00Z",
            refCommitId: commitId,
            contentLoaded: true,
            document: .init(title: path, path: path, body: "body-\(id)")
        )
    }

    private func orgResource(
        id: String,
        path: String? = nil,
        body: String? = nil,
        hash: String? = nil,
        commitId: String = "org-commit",
        contentLoaded: Bool = true
    ) -> MemoryResource {
        let body = body ?? "body-\(id)"
        return MemoryResource(
            id: id,
            scope: .org,
            projectId: nil,
            projectName: nil,
            kind: .context,
            contentHash: hash ?? "hash-\(id)",
            updatedAt: "2026-08-19T00:00:00Z",
            refCommitId: commitId,
            contentLoaded: contentLoaded,
            document: .init(title: id, path: path ?? "\(id).md", body: body)
        )
    }

    private func localDraft(
        id: String,
        targetId: String?,
        projectId: String = "project",
        scope: MemoryScope = .project,
        status: DaemonLocalDraftStatus = .open,
        updatedAt: String = "2026-08-19T00:00:00Z"
    ) -> LocalDraft {
        LocalDraft(
            id: id,
            projectId: projectId,
            serverId: "server-\(id)",
            serverVersion: 1,
            baseCommitId: "commit-old",
            currentCommitId: "commit-new",
            freshness: .behind,
            hasUpstreamResourceChanges: true,
            reconciliation: .unknown,
            reconciliationCandidateId: nil,
            scope: scope,
            kind: .context,
            targetId: targetId,
            status: status,
            origin: .desktop,
            syncStatus: .synced,
            updatedAt: updatedAt,
            document: .init(title: "Memory", path: "memory.md", body: "draft body"),
            isDeletion: false
        )
    }

    private func projectCheckout(
        commitId: String,
        resources: [DaemonProjectCheckoutResource]
    ) -> DaemonProjectCheckout {
        DaemonProjectCheckout(
            projectId: "project",
            commitId: commitId,
            refEtag: "\"\(commitId)\"",
            commitCreatedAt: "2026-08-19T01:00:00Z",
            orgSelectionRevision: 1,
            selectedOrgResourceIds: resources.filter { $0.scope == .org }.map(\.resourceId),
            resources: resources,
            ready: true
        )
    }

    private func checkoutResource(
        id: String,
        path: String,
        hash: String,
        scope: DaemonDraftScope = .project,
        body: String? = nil
    ) -> DaemonProjectCheckoutResource {
        DaemonProjectCheckoutResource(
            resourceId: id,
            scope: scope,
            resourceKind: .memory,
            projectId: scope == .project ? "project" : nil,
            path: path,
            contentHash: hash,
            content: .init(description: nil, content: body ?? "remote-\(id)")
        )
    }

    private func contentHash(_ content: String) -> String {
        let digest = SHA256.hash(data: Data(content.utf8))
        return "sha256:" + digest.map { String(format: "%02x", $0) }.joined()
    }

    private func reviewRecord(
        status: String,
        freshness: DraftFreshness = .current,
        reconciliation: DraftReconciliationStatus = .clean,
        approvedResultHash: String? = nil,
        id: String = UUID().uuidString,
        version: Int = 1,
        currentCommitId: String? = nil,
        projectId: String = "prj",
        authorId: String = "u"
    ) -> ReviewRecord {
        ReviewRecord(
            id: id,
            projectId: projectId,
            draftId: "draft",
            title: "Review title",
            description: "",
            author: UserReference(
                userId: authorId,
                email: "\(authorId)@example.com",
                displayName: authorId == "u" ? "Reviewer" : authorId.capitalized,
                avatarUrl: nil,
                role: "member"
            ),
            status: status,
            version: version,
            decisionBody: nil,
            approvedResultHash: approvedResultHash,
            decidedBy: nil,
            decidedAt: nil,
            freshness: freshness,
            reconciliation: reconciliation,
            reconciliationCandidateId: nil,
            currentCommitId: currentCommitId,
            updatedAt: "2026-08-09T00:00:00Z"
        )
    }

}

private actor WorkspaceNavigationTestLatch {
    private var isOpen = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        guard !isOpen else { return }
        await withCheckedContinuation { continuation in
            waiters.append(continuation)
        }
    }

    func open() {
        isOpen = true
        let pending = waiters
        waiters.removeAll()
        pending.forEach { $0.resume() }
    }
}
