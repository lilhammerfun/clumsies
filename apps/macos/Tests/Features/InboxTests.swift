import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class InboxTests: XCTestCase {
    @MainActor private final class Responses {
        var version = 1
        var unavailable = false
        var pending = 0
    }

    private func context() -> WorkspaceContext {
        let context = WorkspaceContext()
        context.phase = .ready
        context.account = .init(userId: "user", email: "user@example.com", displayName: nil, avatarUrl: nil, role: "member")
        context.organization = .init(orgId: "org", name: "Organization")
        context.projects = [.init(id: "project", name: "Project", refCommitId: nil, refEtag: "ref-none",
            selectedOrgResourceIds: [], orgSelectionRevision: 0, isLoaded: true)]
        return context
    }

    private func preferences() throws -> UserDefaults {
        let suite = "InboxTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        addTeardownBlock { defaults.removePersistentDomain(forName: suite) }
        return defaults
    }

    private func snapshot(failed: Bool = false, pending: Int = 0, behind: Int = 0) -> DaemonSyncStatus {
        let channel = DaemonSyncChannelStatus(state: failed ? "failed" : "idle", serverCursor: nil,
            lastAttemptAt: nil, lastSuccessAt: nil, lastError: nil)
        return .init(draftSync: channel, commitSync: channel,
            pendingOperationCount: pending, failedOperationCount: failed ? 1 : 0,
            behindDraftCount: behind, reconciliationConflictCount: 0, lastSuccessAt: nil)
    }

    private func notification(_ id: String = "review:one", version: Int = 1, kind: String = "review_comment") -> InboxNotification {
        .init(notificationId: id, projectId: "project", projectName: "Project", kind: kind,
            targetId: "one", title: "A comment", actorName: "Reviewer", version: version,
            readVersion: 0, archivedVersion: 0, needsAction: false, reviewStatus: "open", occurredAt: "2026-09-20T00:00:00Z")
    }

    func testToolbarKeepsFiltersLeadingAndFocusesTheTrailingNativeSearch() async throws {
        let context = context()
        let store = InboxStore(context: context, defaults: try preferences(),
            fetchPage: { _ in (.init(items: [self.notification()], nextCursor: nil), false) },
            fetchLocal: { self.snapshot(failed: true) })
        store.prepare(serverURL: "test")
        func root(_ token: UUID) -> some View {
            NavigationSplitView {
                Text("Inbox").navigationSplitViewColumnWidth(220)
            } detail: {
                InboxView(store: store, searchFocusToken: token, open: { _ in })
            }
        }
        let hosting = NSHostingView(rootView: root(UUID()))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 820),
            styleMask: [.titled, .closable, .resizable, .fullSizeContentView], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = hosting
        window.makeKeyAndOrderFront(nil)
        defer { window.close() }
        hosting.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(250))

        let items = try XCTUnwrap(window.toolbar).items
        func item(_ suffix: String) throws -> NSToolbarItem {
            try XCTUnwrap(items.first { $0.itemIdentifier.rawValue.contains("inbox.\(suffix)") })
        }
        func searchField(in view: NSView) -> NSSearchField? {
            if let field = view as? NSSearchField { return field }
            return view.subviews.lazy.compactMap { searchField(in: $0) }.first
        }
        let search = try XCTUnwrap(searchField(in: XCTUnwrap(item("search").view)))
        XCTAssertEqual(search.accessibilityIdentifier(), "inbox-toolbar-search")
        XCTAssertEqual(search.placeholderString, "Search Inbox")
        for (leading, trailing) in [("filter", "type"), ("type", "read"), ("read", "archive"), ("archive", "refresh"), ("refresh", "search")] {
            let left = try XCTUnwrap(item(leading).view)
            let right = try XCTUnwrap(item(trailing).view)
            XCTAssertLessThan(left.convert(left.bounds, to: nil).midX,
                right.convert(right.bounds, to: nil).midX, "\(leading) must precede \(trailing).")
        }

        func table(in view: NSView) -> NSTableView? {
            if let table = view as? NSTableView { return table }
            return view.subviews.lazy.compactMap { table(in: $0) }.first
        }
        let list = try XCTUnwrap(table(in: hosting))
        let unread = store.unreadCount
        list.selectRowIndexes(IndexSet(integersIn: 0..<2), byExtendingSelection: false)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(list.selectedRowIndexes.count, 2, "The native list must support multi-selection.")
        XCTAssertEqual(store.unreadCount, unread, "Selection alone must not mark notifications read.")

        window.makeFirstResponder(nil)
        hosting.rootView = root(UUID())
        hosting.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(250))
        XCTAssertNotNil(search.currentEditor(), "The workspace search command must focus Inbox search.")
    }

    func testRowsStayTwoLinesAtNarrowAndWideWindowSizes() throws {
        let notice = InboxItem.server(notification())
        for width in [540.0, 1100.0] {
            let row = NSHostingView(rootView: InboxRow(item: notice, isOpening: false, activate: {})
                .frame(width: width))
            XCTAssertLessThanOrEqual(row.fittingSize.height, 55, "Rows must reserve only two text lines, including their padding.")
        }
        let informational = InboxItem(id: "info", type: .syncErrors, projectId: nil, projectName: "This Mac",
            title: "Sync complete", message: "All changes are up to date.", occurredAt: Date(),
            needsAction: false, revision: "1", isRead: false, isArchived: false, destination: nil)
        XCTAssertNil(informational.actionTitle, "Information without a destination must not offer a fake detail page.")
    }

    func testReadUnreadAndArchiveAreIndependentAndBatchArchiveCanBeUndone() async throws {
        let context = context(), defaults = try preferences()
        var requests: [InboxReceiptAction] = []
        let store = InboxStore(context: context, defaults: defaults,
            fetchPage: { _ in (.init(items: [self.notification()], nextCursor: nil), false) },
            fetchLocal: { self.snapshot(failed: true) },
            updateReceipt: { _, request in requests.append(request.action) })
        store.prepare(serverURL: "test")
        await store.refresh()
        let original = store.items
        let undo = UndoManager()
        undo.groupsByEvent = false
        undo.beginUndoGrouping()
        await store.acknowledge(original, action: .archive, undoManager: undo)
        undo.endUndoGrouping()
        XCTAssertTrue(store.items.allSatisfy { $0.isArchived && !$0.isRead })
        XCTAssertEqual(store.unreadCount, 0)
        XCTAssertTrue(undo.canUndo)
        undo.undo()
        // Undo sends an asynchronous receipt, including for server-backed messages.
        for _ in 0..<20 where store.items.contains(where: \.isArchived) {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertTrue(store.items.allSatisfy { !$0.isArchived && !$0.isRead })
        XCTAssertEqual(store.unreadCount, 2)
        await store.acknowledge(original, action: .read, undoManager: nil)
        XCTAssertEqual(store.unreadCount, 0)
        await store.acknowledge(original, action: .unread, undoManager: nil)
        XCTAssertEqual(store.unreadCount, 2)
        XCTAssertEqual(requests, [.archive, .restore, .read, .unread])

        let restarted = InboxStore(context: context, defaults: defaults,
            fetchPage: { _ in (.init(items: [], nextCursor: nil), false) },
            fetchLocal: { self.snapshot(failed: true) })
        restarted.prepare(serverURL: "test")
        await restarted.refresh()
        XCTAssertEqual(restarted.unreadCount, 1, "Mark as Unread must persist for local reminders too.")
    }

    func testBatchStopsAfterReceiptFailureAndUndoCannotCrossAccountReset() async throws {
        let context = context()
        var count = 0
        let store = InboxStore(context: context, defaults: try preferences(),
            fetchPage: { _ in (.init(items: [self.notification("one"), self.notification("two")], nextCursor: nil), false) },
            fetchLocal: { self.snapshot() }, updateReceipt: { _, _ in
                count += 1
                if count == 2 { throw URLError(.notConnectedToInternet) }
            })
        store.prepare(serverURL: "test")
        await store.refresh()
        let undo = UndoManager()
        undo.groupsByEvent = false
        undo.beginUndoGrouping()
        await store.acknowledge(store.items, action: .archive, undoManager: undo)
        undo.endUndoGrouping()
        XCTAssertEqual(store.items.filter(\.isArchived).count, 1)
        XCTAssertNotNil(store.receiptError)
        context.authorityGeneration = UUID()
        undo.undo()
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertEqual(count, 2, "Undo from another account must not send receipts.")
        XCTAssertTrue(store.items.isEmpty)
    }

    func testMessageTypesDistinguishReviewEventsAndLocalReminders() {
        let expected: [String: InboxMessageType] = [
            "review_requested": .reviewRequests, "review_comment": .reviewComments,
            "review_approved": .reviewResults, "review_rejected": .reviewResults,
            "review_merged": .reviewResults, "shared_update": .sharedUpdates,
        ]
        for (kind, type) in expected {
            XCTAssertEqual(InboxItem.server(notification(kind: kind)).type, type)
        }
        let local = InboxStore.localNotifications(snapshot(failed: true))
        XCTAssertEqual(Set(local.map(\.type)), [.syncErrors])
    }

    func testActivationOpensExistingDestinationsDirectlyAndReadsOnlyAfterSuccess() async throws {
        let context = context()
        var opened: [InboxDestination] = []
        var receipts: [InboxReceiptAction] = []
        let store = InboxStore(context: context, defaults: try preferences(),
            fetchPage: { _ in (.init(items: [self.notification(), self.notification("shared:project", kind: "shared_update")], nextCursor: nil), false) },
            fetchLocal: { self.snapshot() },
            updateReceipt: { _, request in
                XCTAssertFalse(Task.isCancelled, "A successful navigation must finish its read receipt.")
                receipts.append(request.action)
            })
        store.prepare(serverURL: "test")
        await store.refresh()
        let shared = try XCTUnwrap(store.items.first { $0.type == .sharedUpdates })
        XCTAssertEqual(shared.actionTitle, "Open Memory")
        do {
            try await store.activate(shared, open: { _ in throw URLError(.cannotConnectToHost) })
            XCTFail("Failed navigation must remain retryable.")
        } catch is URLError { }
        XCTAssertEqual(store.unreadCount, 2)

        let single = try XCTUnwrap(store.items.first { $0.type == .reviewComments })
        let action = Task {
            try await store.activate(single, open: {
                opened.append($0)
                withUnsafeCurrentTask { $0?.cancel() } // Leaving Inbox cancels its view task.
            })
        }
        try await action.value
        XCTAssertEqual(opened, [.review("one")])
        XCTAssertEqual(receipts, [.read])
        XCTAssertEqual(store.unreadCount, 1)

        try await store.activate(shared, open: {
            XCTAssertEqual(store.unreadCount, 1, "Navigation must finish before acknowledging the notification.")
            opened.append($0)
        })
        XCTAssertEqual(opened, [.review("one"), .sharedChanges(projectId: "project")])
        XCTAssertEqual(receipts, [.read, .read])
        XCTAssertEqual(store.unreadCount, 0)
    }

    func testAuthorityChangeDuringNavigationCannotAcknowledgeOldMessages() async throws {
        let context = context()
        var didAcknowledge = false
        let store = InboxStore(context: context, defaults: try preferences(),
            fetchPage: { _ in (.init(items: [self.notification()], nextCursor: nil), false) },
            fetchLocal: { self.snapshot() }, updateReceipt: { _, _ in didAcknowledge = true })
        store.prepare(serverURL: "test")
        await store.refresh()
        let item = try XCTUnwrap(store.items.first)
        let started = expectation(description: "Opening message destination")
        var pending: CheckedContinuation<Void, Error>?
        let action = Task {
            try await store.activate(item, open: { _ in
                try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
            })
        }
        await fulfillment(of: [started], timeout: 1)
        context.authorityGeneration = UUID()
        try XCTUnwrap(pending).resume()
        try await action.value
        XCTAssertFalse(didAcknowledge)
        XCTAssertTrue(store.items.isEmpty)
    }

    func testOrdinaryDraftActivityDoesNotCreateNotifications() async throws {
        let context = context()
        let responses = Responses()
        let store = InboxStore(context: context, defaults: try preferences(),
            fetchPage: { _ in (.init(items: [], nextCursor: nil), false) },
            fetchLocal: { self.snapshot(pending: responses.pending, behind: 3) })
        store.prepare(serverURL: "test")
        for pending in [0, 1, 5, 0] {
            responses.pending = pending
            await store.refresh()
            XCTAssertTrue(store.items.isEmpty, "Normal draft uploads and a behind base ref are not notifications.")
            XCTAssertEqual(store.unreadCount, 0)
        }
    }

    func testSyncReceiptsSurviveRestartAndResolvedIssuesMayRecur() async throws {
        let context = context(), defaults = try preferences()
        var local = snapshot(failed: true)
        func makeStore() -> InboxStore {
            let store = InboxStore(context: context, defaults: defaults,
                fetchPage: { _ in (.init(items: [], nextCursor: nil), false) }, fetchLocal: { local })
            store.prepare(serverURL: "https://example.com")
            return store
        }
        let store = makeStore()
        await store.refresh()
        let initial = try XCTUnwrap(store.items.first)
        await store.acknowledge(initial, action: .archive)
        let reopened = makeStore()
        await reopened.refresh()
        XCTAssertTrue(try XCTUnwrap(reopened.items.first).isArchived)
        local = snapshot()
        await reopened.refresh()
        XCTAssertTrue(reopened.items.isEmpty)
        local = snapshot(failed: true)
        await reopened.refresh()
        XCTAssertEqual(reopened.unreadCount, 1)
        await reopened.acknowledge(try XCTUnwrap(reopened.items.first), action: .archive)
        reopened.prepare(serverURL: "https://another.example.com")
        await reopened.refresh()
        XCTAssertEqual(reopened.unreadCount, 1, "Receipt scope includes the server.")
    }

    func testServerPaginationAndDelayedReceiptsCannotHideNewerEvents() async throws {
        let context = context(), defaults = try preferences()
        let responses = Responses()
        var requests: [String?] = []
        let started = expectation(description: "Receipt started")
        var pending: CheckedContinuation<Void, Error>?
        let store = InboxStore(context: context, defaults: defaults, fetchPage: { cursor in
            requests.append(cursor)
            return (.init(items: [self.notification(cursor == nil ? "review:one" : "review:two", version: responses.version)],
                          nextCursor: cursor == nil ? "next" : nil), false)
        }, fetchLocal: { self.snapshot() }, updateReceipt: { _, request in
            XCTAssertEqual(request.version, 1)
            try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
        })
        store.prepare(serverURL: "test")
        await store.refresh()
        XCTAssertEqual(requests, [nil, "next"])
        let old = try XCTUnwrap(store.items.first)
        let acknowledgement = Task { await store.acknowledge(old, action: .archive) }
        await fulfillment(of: [started], timeout: 1)
        responses.version = 2
        await store.refresh()
        try XCTUnwrap(pending).resume()
        _ = await acknowledgement.value
        XCTAssertEqual(store.unreadCount, 2)
        XCTAssertFalse(try XCTUnwrap(store.items.first).isArchived)
        context.projects = []
        XCTAssertTrue(store.items.isEmpty, "Revoked project access clears displayed notifications immediately.")
    }

    func testAnInFlightRefreshDoesNotUndoAConfirmedRead() async throws {
        let context = context(), defaults = try preferences()
        let started = expectation(description: "Old refresh started")
        var pending: CheckedContinuation<(InboxPage, Bool), Error>?
        var calls = 0
        let page = InboxPage(items: [notification()], nextCursor: nil)
        let store = InboxStore(context: context, defaults: defaults, fetchPage: { _ in
            calls += 1
            if calls == 2 {
                return try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
            }
            return (page, false)
        }, fetchLocal: { self.snapshot() }, updateReceipt: { _, _ in })
        store.prepare(serverURL: "test")
        await store.refresh()
        let refresh = Task { await store.refresh() }
        await fulfillment(of: [started], timeout: 1)
        await store.acknowledge(try XCTUnwrap(store.items.first), action: .read)
        try XCTUnwrap(pending).resume(returning: (page, false))
        await refresh.value
        XCTAssertEqual(store.unreadCount, 0)
        XCTAssertFalse(try XCTUnwrap(store.items.first).isArchived)
    }

    func testAuthorityResetRejectsLateContentAndLateErrors() async throws {
        for succeeds in [true, false] {
            let context = context(), defaults = try preferences()
            let started = expectation(description: "Refresh started")
            var pending: CheckedContinuation<(InboxPage, Bool), Error>?
            let store = InboxStore(context: context, defaults: defaults, fetchPage: { _ in
                try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
            }, fetchLocal: { self.snapshot(failed: true) })
            store.prepare(serverURL: "test")
            let old = Task { await store.refresh() }
            await fulfillment(of: [started], timeout: 1)
            context.authorityGeneration = UUID()
            if succeeds {
                try XCTUnwrap(pending).resume(returning: (.init(items: [notification()], nextCursor: nil), false))
            } else { try XCTUnwrap(pending).resume(throwing: URLError(.timedOut)) }
            await old.value
            XCTAssertTrue(store.items.isEmpty)
            XCTAssertNil(store.errorMessage)
            XCTAssertFalse(store.isLoading)
            XCTAssertFalse(store.hasLoaded)
        }
    }

    func testExistingSharedChangesRemainDiscoverableWithoutAServerNotification() async throws {
        let context = context(), defaults = try preferences()
        let catalog = MemoryCatalog(context: context)
        catalog.staleResourceSnapshots["file"] = .init(projectId: "project", observedProjectRefCommitId: nil,
            observedSelectedOrgResourceIds: [], observedOrgSelectionRevision: 0, authoritativeCommitId: "new",
            authoritativeRefEtag: nil, selectedOrgResourceIds: [], orgSelectionRevision: 0,
            generation: UUID(), local: nil, remote: nil)
        let store = InboxStore(context: context, catalog: catalog, defaults: defaults,
            fetchPage: { _ in (.init(items: [], nextCursor: nil), false) }, fetchLocal: { self.snapshot() })
        store.prepare(serverURL: "test")
        await store.refresh()
        XCTAssertEqual(store.items.first?.type, .sharedUpdates)
        XCTAssertEqual(store.items.first?.destination, .sharedChanges(projectId: "project"))
        await store.acknowledge(try XCTUnwrap(store.items.first), action: .archive)
        await store.refresh()
        XCTAssertEqual(store.unreadCount, 0)
        catalog.staleResourceSnapshots = [:]
        await store.refresh()
        XCTAssertTrue(store.items.isEmpty)
    }

    func testOfflineReceiptDoesNotPretendSuccessAndLocalServiceFailureRemainsDiscoverable() async throws {
        let context = context(), defaults = try preferences()
        let responses = Responses()
        let store = InboxStore(context: context, defaults: defaults,
            fetchPage: { _ in (.init(items: [self.notification()], nextCursor: nil), true) },
            fetchLocal: {
                if responses.unavailable { throw URLError(.cannotConnectToHost) }
                return self.snapshot(failed: true)
            }, updateReceipt: { _, _ in throw URLError(.notConnectedToInternet) })
        store.prepare(serverURL: "test")
        await store.refresh()
        XCTAssertTrue(store.isShowingSavedContent)
        let remote = try XCTUnwrap(store.items.first { $0.serverVersion != nil })
        await store.acknowledge(remote, action: .archive)
        XCTAssertNotNil(store.receiptError)
        XCTAssertFalse(try XCTUnwrap(store.items.first { $0.id == remote.id }).isArchived)
        responses.unavailable = true
        await store.refresh()
        XCTAssertNotNil(store.items.first { $0.id == "local:unavailable" })
        XCTAssertNotNil(store.items.first { $0.id == "local:sync" }, "A local fetch failure retains known sync issues.")
        responses.unavailable = false
        await store.refresh()
        XCTAssertNil(store.items.first { $0.id == "local:unavailable" })
    }
}
