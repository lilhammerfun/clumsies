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
        var online = false
    }

    override func setUp() async throws {
        ClientServiceStatus.shared.reset()
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
            readVersion: 0, archivedVersion: 0, needsAction: false, reviewStatus: "open", occurredAt: "2026-09-20T00:00:00Z",
            body: nil, previousRole: nil, newRole: nil, canOpenProject: true)
    }

    private func accountNotification(_ kind: String, canOpenProject: Bool = true) -> InboxNotification {
        .init(notificationId: kind, projectId: kind == "welcome" || kind == "org_role_changed" ? nil : "project",
            projectName: kind == "welcome" || kind == "org_role_changed" ? nil : "Project", kind: kind,
            targetId: "project", title: "Clumsies", actorName: "Owner", version: 1,
            readVersion: 0, archivedVersion: 0, needsAction: false, reviewStatus: nil, occurredAt: "2026-09-21T00:00:00Z",
            body: kind == "welcome" ? "## Your projects\n\nOpen Memory to get started." : nil,
            previousRole: kind == "project_joined" ? nil : "member", newRole: kind == "project_removed" ? nil : "admin",
            canOpenProject: canOpenProject)
    }

    func testWelcomeHasReadableContentAndAccessMessagesUseExistingDestinations() throws {
        let welcome = InboxItem.server(accountNotification("welcome"))
        XCTAssertEqual(welcome.type, .welcome)
        XCTAssertNotNil(welcome.body)
        XCTAssertEqual(welcome.actionTitle, "Read Message")
        XCTAssertNil(welcome.destination)
        for kind in ["project_joined", "project_role_changed", "project_removed", "org_role_changed"] {
            let item = InboxItem.server(accountNotification(kind))
            XCTAssertEqual(item.type, .accessChanges)
            XCTAssertNil(item.body, "Access events should not invent a message detail page.")
            XCTAssertEqual(item.destination, kind == "project_joined" || kind == "project_role_changed" ? .project("project") : nil)
            let inaccessible = InboxItem.server(accountNotification(kind, canOpenProject: false))
            XCTAssertNil(inaccessible.destination)
            XCTAssertNil(inaccessible.actionTitle)
        }
        let changed = InboxItem.server(accountNotification("project_role_changed"))
        XCTAssertTrue(changed.message.contains("Member → Admin"))
        let detail = NSHostingView(rootView: InboxMessageView(item: welcome, projectId: nil, open: { _ in
            XCTFail("A welcome without projects must not attempt navigation.")
        }).frame(width: 800, height: 700))
        XCTAssertEqual(detail.fittingSize, NSSize(width: 800, height: 700), "Message content should fill its navigation destination.")
        for notice in [welcome, changed, InboxItem.server(accountNotification("project_removed"))] {
            let row = NSHostingView(rootView: InboxRow(item: notice, isOpening: false, activate: {}).frame(width: 540))
            XCTAssertLessThanOrEqual(row.fittingSize.height, 55)
        }
    }

    func testPersonalNoticesSurviveProjectRemovalButLoseProjectActions() async throws {
        let context = context()
        var receipts: [String] = []
        let notices = [accountNotification("welcome"), accountNotification("project_joined"),
            accountNotification("project_removed"), notification()]
        let store = InboxStore(context: context, defaults: try preferences(),
            fetchPage: { _ in (.init(items: notices, nextCursor: nil), false) },
            fetchLocal: { self.snapshot() }, updateReceipt: { id, _ in receipts.append(id) })
        store.prepare(serverURL: "test")
        await store.refresh()
        XCTAssertEqual(store.items.count, 4)
        XCTAssertEqual(store.items.first(where: { $0.id == "project_joined" })?.destination, .project("project"))
        context.projects = []
        await Task.yield()
        XCTAssertEqual(Set(store.items.map(\.id)), ["welcome", "project_joined", "project_removed"])
        XCTAssertTrue(store.items.allSatisfy { $0.destination == nil })
        XCTAssertNil(store.welcomeProjectId)
        await store.refresh()
        XCTAssertEqual(store.items.count, 3, "Cached source content must stay hidden after access is revoked.")
        let removed = try XCTUnwrap(store.items.first { $0.id == "project_removed" })
        let archived = await store.acknowledge(removed, action: .archive)
        XCTAssertTrue(archived)
        XCTAssertEqual(receipts, ["project_removed"])
        XCTAssertFalse(try XCTUnwrap(store.items.first { $0.id == "project_removed" }).isRead)
        let welcome = try XCTUnwrap(store.items.first { $0.id == "welcome" })
        let read = await store.acknowledge(welcome, action: .read)
        XCTAssertTrue(read)
        XCTAssertFalse(try XCTUnwrap(store.items.first { $0.id == "welcome" }).isArchived)
        context.authorityGeneration = UUID()
        XCTAssertTrue(store.items.isEmpty)
    }

    func testToolbarKeepsFiltersLeadingAndFocusesTheTrailingNativeSearch() async throws {
        let context = context()
        var receipts: [InboxReceiptAction] = []
        let store = InboxStore(context: context, defaults: try preferences(),
            fetchPage: { _ in (.init(items: [self.notification()], nextCursor: nil), false) },
            fetchLocal: { self.snapshot(failed: true) },
            updateReceipt: { _, request in receipts.append(request.action) })
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
        XCTAssertEqual(store.unreadCount, unread, "Bulk selection must preserve unread messages for archive actions.")

        let serverRow = try XCTUnwrap(store.items.firstIndex { $0.serverVersion != nil })
        list.selectRowIndexes(IndexSet(integer: serverRow), byExtendingSelection: false)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertTrue(receipts.isEmpty, "Selection alone must not change read status.")
        XCTAssertEqual(store.unreadCount, unread)
        let selectedItem = try XCTUnwrap(store.items.first { $0.serverVersion != nil })
        await store.acknowledge(selectedItem, action: .read)
        await store.acknowledge(selectedItem, action: .unread)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(store.unreadCount, unread, "Explicit Mark as Unread must remain effective while the row stays selected.")

        window.makeFirstResponder(nil)
        hosting.rootView = root(UUID())
        hosting.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(250))
        XCTAssertNotNil(search.currentEditor(), "The workspace search command must focus Inbox search.")
    }

    private func click(_ point: NSPoint, in window: NSWindow) throws {
        let timestamp = ProcessInfo.processInfo.systemUptime
        let up = try XCTUnwrap(NSEvent.mouseEvent(with: .leftMouseUp, location: point, modifierFlags: [],
            timestamp: timestamp, windowNumber: window.windowNumber, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 0))
        let down = try XCTUnwrap(NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [],
            timestamp: timestamp, windowNumber: window.windowNumber, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 1))
        NSApp.postEvent(up, atStart: false)
        window.sendEvent(down)
    }

    func testCheckboxSelectionDoesNotReadAndToolbarArchivesTheWholeSelection() async throws {
        var receipts: [(String, InboxReceiptAction)] = []
        let store = InboxStore(context: context(), defaults: try preferences(),
            fetchPage: { _ in (.init(items: [self.notification("one"), self.notification("two")], nextCursor: nil), false) },
            fetchLocal: { self.snapshot() }, updateReceipt: { id, request in receipts.append((id, request.action)) })
        store.prepare(serverURL: "test")
        let host = NSHostingView(rootView: InboxView(store: store, searchFocusToken: UUID(), open: { _ in
            XCTFail("Selecting or archiving must not open a destination.")
        }))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1100, height: 700),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.makeKeyAndOrderFront(nil)
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(250))
        func table(in view: NSView) -> NSTableView? {
            if let table = view as? NSTableView { return table }
            return view.subviews.lazy.compactMap { table(in: $0) }.first
        }
        let list = try XCTUnwrap(table(in: host))
        func toggle(_ row: Int) throws {
            let rect = list.rect(ofRow: row)
            try click(list.convert(NSPoint(x: rect.minX + 18, y: rect.minY + 17), to: nil), in: window)
        }
        try toggle(0)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(list.selectedRowIndexes, IndexSet(integer: 0))
        XCTAssertEqual(store.unreadCount, 2)
        XCTAssertTrue(receipts.isEmpty)
        try toggle(1)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(list.selectedRowIndexes.count, 2)
        XCTAssertTrue(receipts.isEmpty, "Checkboxes must only change selection, including its first item.")
        try toggle(0)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(list.selectedRowIndexes, IndexSet(integer: 1))
        XCTAssertTrue(receipts.isEmpty, "Deselecting down to one item must not read the remaining item.")
        try toggle(0)
        try await Task.sleep(for: .milliseconds(100))
        let archive = try XCTUnwrap(window.toolbar?.items.first { $0.itemIdentifier.rawValue.contains("inbox.archive") }?.view)
        let point = archive.convert(NSPoint(x: archive.bounds.midX, y: archive.bounds.midY), to: nil)
        try click(point, in: window)
        for _ in 0..<40 where receipts.count != 2 || !store.items.allSatisfy(\.isArchived) {
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertEqual(Set(receipts.map(\.0)), ["one", "two"])
        XCTAssertTrue(receipts.allSatisfy { $0.1 == .archive })
        XCTAssertTrue(store.items.allSatisfy(\.isArchived))
        XCTAssertEqual(store.unreadCount, 0)
        XCTAssertTrue(store.items.allSatisfy { !$0.isRead }, "Batch archiving must preserve unread state.")
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

    func testWelcomeOpensInsideMainWindowAndMarksReadWithoutASheet() async throws {
        let store = InboxStore(context: context(), defaults: try preferences(),
            fetchPage: { _ in (.init(items: [self.accountNotification("welcome")], nextCursor: nil), false) },
            fetchLocal: { self.snapshot() }, updateReceipt: { _, _ in })
        store.prepare(serverURL: "test")
        await store.refresh()
        let host = NSHostingView(rootView: NavigationSplitView {
            Text("Inbox").navigationSplitViewColumnWidth(220)
        } detail: {
            InboxView(store: store, searchFocusToken: UUID(), open: { _ in })
        })
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 700),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.makeKeyAndOrderFront(nil)
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(250))
        func find<V: NSView>(_ type: V.Type, in view: NSView) -> V? {
            if let found = view as? V { return found }
            return view.subviews.lazy.compactMap { find(type, in: $0) }.first
        }
        let list = try XCTUnwrap(find(NSTableView.self, in: host))
        list.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        let row = list.rect(ofRow: 0)
        let point = list.convert(NSPoint(x: row.maxX - 60, y: row.maxY - 15), to: nil)
        let mouseUp = try XCTUnwrap(NSEvent.mouseEvent(with: .leftMouseUp, location: point, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
            context: nil, eventNumber: 1, clickCount: 1, pressure: 0))
        let mouseDown = try XCTUnwrap(NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
            context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        // AppKit buttons track synchronously; make the release available to that loop.
        NSApp.postEvent(mouseUp, atStart: false)
        window.sendEvent(mouseDown)
        let expectedTitle = InboxItem.server(accountNotification("welcome")).title
        // Native navigation and the destination's read acknowledgement finish independently.
        for _ in 0..<40 where window.title != expectedTitle || store.unreadCount != 0 {
            try await Task.sleep(for: .milliseconds(50))
        }
        host.layoutSubtreeIfNeeded()
        XCTAssertTrue(window.sheets.isEmpty, "Reading a welcome message must not block the workspace with a sheet.")
        XCTAssertEqual(window.title, expectedTitle)
        XCTAssertEqual(store.unreadCount, 0)
        XCTAssertFalse(window.toolbar?.items.contains { $0.itemIdentifier.rawValue.contains("inbox.filter") } ?? false)
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        let attachment = XCTAttachment(data: try XCTUnwrap(bitmap.representation(using: .png, properties: [:])),
            uniformTypeIdentifier: "public.png")
        attachment.name = "Inbox welcome navigation"
        attachment.lifetime = .keepAlways
        add(attachment)
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
                if count == 2 { throw ServerClientError.response(status: 403, message: "PRIVATE_RESPONSE") }
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

    func testUnavailableLocalProjectsRemainManageableAndResolveWithoutRetryErrors() async throws {
        let context = context(), defaults = try preferences()
        let binding = DaemonProjectBinding(serverUrl: "https://example.com", workspaceRoot: "/repos/removed",
            projectId: "removed", revision: 1, createdAt: "", updatedAt: "")
        var local = snapshot()
        local.unavailableProjects = [.init(projectId: "removed", bindings: [binding], draftCount: 2)]
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let decoded = try decoder.decode(DaemonSyncStatus.self, from: encoder.encode(local))
        XCTAssertEqual(decoded.unavailableProjects, local.unavailableProjects)
        let store = InboxStore(context: context, defaults: defaults,
            fetchPage: { _ in (.init(items: [], nextCursor: nil), false) }, fetchLocal: { local })
        store.prepare(serverURL: "https://example.com")
        await store.refresh()
        let notice = try XCTUnwrap(store.items.first)
        XCTAssertEqual(store.items.count, 1)
        XCTAssertEqual(notice.id, "local:project:removed")
        XCTAssertEqual(notice.projectName, "removed")
        XCTAssertEqual(notice.destination, .manageLocalProjects)
        XCTAssertEqual(notice.actionTitle, "Manage Unavailable Projects")
        XCTAssertFalse(store.items.contains { $0.id == "local:sync" })
        let view = NSHostingView(rootView: LocalProjectRecoveryView(store: store, retry: { .completed }))
        XCTAssertEqual(view.fittingSize.height, 420)
        context.projects = []
        await Task.yield()
        XCTAssertEqual(store.items.count, 1, "Recovery must stay reachable without remote project access.")
        await store.acknowledge(notice, action: .archive)
        await store.refresh()
        XCTAssertTrue(try XCTUnwrap(store.items.first).isArchived, "Periodic retries must not create new notices.")
        local.unavailableProjects = []
        await store.refresh()
        XCTAssertTrue(store.items.isEmpty)
        XCTAssertTrue(store.unavailableProjects.isEmpty)
        local.unavailableProjects = [.init(projectId: "removed", bindings: [binding], draftCount: 2)]
        await store.refresh()
        XCTAssertEqual(store.unreadCount, 1, "A later loss of access is actionable again.")
        context.authorityGeneration = UUID()
        XCTAssertTrue(store.unavailableProjects.isEmpty)
    }

    func testLocalProjectRecoveryActionsStayAtTheBottomWhenTheListEmpties() async throws {
        let context = context(), responses = Responses()
        responses.unavailable = true
        let store = InboxStore(context: context, defaults: try preferences(),
            fetchPage: { _ in (.init(items: [], nextCursor: nil), false) }, fetchLocal: {
                var local = self.snapshot()
                if responses.unavailable {
                    local.unavailableProjects = [.init(projectId: "removed", bindings: [], draftCount: 1)]
                }
                return local
            })
        store.prepare(serverURL: "test")
        await store.refresh()
        let retried = expectation(description: "Retry displays an error")
        let controller = NSHostingController(rootView: LocalProjectRecoveryView(store: store, retry: {
            retried.fulfill()
            return .failed("The server is unavailable. Try again when connected.")
        })
            .background(Color(nsColor: .windowBackgroundColor))
            .environment(\.controlActiveState, .inactive))
        let host = controller.view
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 620, height: 420),
            styleMask: [], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentViewController = controller
        window.makeKeyAndOrderFront(nil)
        defer { window.close() }
        func footerSnapshot(_ name: String) async throws -> Data {
            host.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(100))
            host.layoutSubtreeIfNeeded()
            let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: bitmap)
            let attachment = XCTAttachment(data: try XCTUnwrap(bitmap.representation(using: .png, properties: [:])),
                uniformTypeIdentifier: "public.png")
            attachment.name = name
            attachment.lifetime = .keepAlways
            add(attachment)
            let image = try XCTUnwrap(bitmap.cgImage)
            let height = CGFloat(image.height) * 56 / host.bounds.height
            let footer = try XCTUnwrap(image.cropping(to: CGRect(x: 0, y: CGFloat(image.height) - height,
                width: CGFloat(image.width), height: height)))
            return try XCTUnwrap(NSBitmapImageRep(cgImage: footer).representation(using: .png, properties: [:]))
        }
        let before = try await footerSnapshot("Unavailable Projects - With projects")
        responses.unavailable = false
        await store.refresh()
        let after = try await footerSnapshot("Unavailable Projects - Empty")
        XCTAssertTrue(before == after, "Clearing the last unavailable project must not move or redraw the footer buttons.")
        let enter = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil, characters: "\r",
            charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36))
        XCTAssertTrue(window.performKeyEquivalent(with: enter))
        await fulfillment(of: [retried], timeout: 1)
        let failed = try await footerSnapshot("Unavailable Projects - Retry failed")
        XCTAssertTrue(after == failed, "Showing a retry error must not move the footer buttons.")
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

    func testOfflineReceiptIsQueuedAndFailedStatusProbeDoesNotInventNotifications() async throws {
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
        XCTAssertNil(store.receiptError)
        XCTAssertTrue(try XCTUnwrap(store.items.first { $0.id == remote.id }).isArchived)
        responses.unavailable = true
        await store.refresh()
        XCTAssertNil(store.items.first { $0.id == "local:unavailable" })
        XCTAssertNotNil(store.items.first { $0.id == "local:sync" }, "A local fetch failure retains known sync issues.")
        responses.unavailable = false
        await store.refresh()
        XCTAssertNil(store.items.first { $0.id == "local:unavailable" })
    }

    func testOfflineReceiptsSurviveRelaunchCoalesceAndDoNotAcknowledgeNewVersions() async throws {
        let context = context(), defaults = try preferences()
        var online = false
        var version = 1
        var sent: [InboxReceiptAction] = []
        func makeStore() -> InboxStore {
            InboxStore(context: context, defaults: defaults,
                fetchPage: { _ in (.init(items: [self.notification(version: version)], nextCursor: nil), !online) },
                fetchLocal: { self.snapshot() }, updateReceipt: { _, receipt in
                    guard online else { throw URLError(.notConnectedToInternet) }
                    sent.append(receipt.action)
                })
        }
        let first = makeStore()
        first.prepare(serverURL: "test")
        await first.refresh()
        let item = try XCTUnwrap(first.items.first)
        let readAccepted = await first.acknowledge(item, action: .read)
        XCTAssertTrue(readAccepted)
        await first.acknowledge(item, action: .unread)
        await first.acknowledge(item, action: .archive)
        XCTAssertTrue(sent.isEmpty)
        let restarted = makeStore()
        restarted.prepare(serverURL: "test")
        await restarted.refresh()
        XCTAssertFalse(try XCTUnwrap(restarted.items.first).isRead)
        XCTAssertTrue(try XCTUnwrap(restarted.items.first).isArchived)
        XCTAssertNil(restarted.receiptError)
        online = true
        version = 2
        await restarted.refresh()
        XCTAssertEqual(sent, [.unread, .archive], "Only the last read intent and last archive intent are replayed.")
        XCTAssertFalse(try XCTUnwrap(restarted.items.first).isRead, "Acknowledging version 1 must not read version 2.")
        XCTAssertFalse(try XCTUnwrap(restarted.items.first).isArchived)
        let afterSync = makeStore()
        afterSync.prepare(serverURL: "test")
        await afterSync.refresh()
        XCTAssertEqual(sent.count, 2, "Confirmed intents must be removed from disk.")
    }

    func testPermanentReceiptFailureRollsBackAndDoesNotRetryAfterRelaunch() async throws {
        let context = context(), defaults = try preferences()
        var attempts = 0
        var status = 404
        func makeStore() -> InboxStore {
            InboxStore(context: context, defaults: defaults,
                fetchPage: { _ in (.init(items: [self.notification()], nextCursor: nil), false) },
                fetchLocal: { self.snapshot() }, updateReceipt: { _, _ in
                    attempts += 1
                    throw ServerClientError.response(status: status, message: "PRIVATE_BODY req_secret")
                })
        }
        let store = makeStore()
        store.prepare(serverURL: "test")
        await store.refresh()
        let accepted = await store.acknowledge(try XCTUnwrap(store.items.first), action: .archive)
        XCTAssertFalse(accepted)
        XCTAssertFalse(try XCTUnwrap(store.items.first).isArchived)
        XCTAssertEqual(store.receiptError, ClientFailure.missing.message)
        let restarted = makeStore()
        restarted.prepare(serverURL: "test")
        await restarted.refresh()
        XCTAssertEqual(attempts, 1)
        status = 400
        let invalid = await restarted.acknowledge(try XCTUnwrap(restarted.items.first), action: .archive)
        XCTAssertFalse(invalid)
        XCTAssertFalse(try XCTUnwrap(restarted.items.first).isArchived)
        XCTAssertEqual(restarted.receiptError, String(localized: "Couldn't update this notification. Refresh Inbox and try again."))
    }

    func testAReadInFlightDoesNotBlockArchiveOrReplaceALaterUnreadIntent() async throws {
        let started = expectation(description: "Receipt started")
        var reply: CheckedContinuation<Void, Error>?
        var sent: [InboxReceiptAction] = []
        let store = InboxStore(context: context(), defaults: try preferences(),
            fetchPage: { _ in (.init(items: [self.notification()], nextCursor: nil), false) },
            fetchLocal: { self.snapshot() }, updateReceipt: { _, request in
                sent.append(request.action)
                if sent.count == 1 {
                    try await withCheckedThrowingContinuation { reply = $0; started.fulfill() }
                }
            })
        store.prepare(serverURL: "test")
        await store.refresh()
        let item = try XCTUnwrap(store.items.first)
        let read = Task { await store.acknowledge(item, action: .read) }
        await fulfillment(of: [started], timeout: 1)
        await store.acknowledge(item, action: .archive)
        await store.acknowledge(item, action: .unread)
        XCTAssertTrue(try XCTUnwrap(store.items.first).isArchived)
        XCTAssertFalse(try XCTUnwrap(store.items.first).isRead)
        try XCTUnwrap(reply).resume()
        _ = await read.value
        XCTAssertFalse(try XCTUnwrap(store.items.first).isRead)
        await store.refresh()
        XCTAssertEqual(sent, [.read, .archive, .unread])
    }

    func testPendingReceiptsNeverReplayIntoAnotherAccount() async throws {
        let context = context(), defaults = try preferences()
        let responses = Responses()
        var sends = 0
        let store = InboxStore(context: context, defaults: defaults,
            fetchPage: { _ in (.init(items: [self.notification()], nextCursor: nil), false) },
            fetchLocal: { self.snapshot() }, updateReceipt: { _, _ in
                guard responses.online else { throw URLError(.notConnectedToInternet) }
                sends += 1
            })
        store.prepare(serverURL: "test")
        await store.refresh()
        await store.acknowledge(try XCTUnwrap(store.items.first), action: .read)
        context.authorityGeneration = UUID()
        context.account = .init(userId: "other", email: "other@example.com", displayName: nil, avatarUrl: nil, role: "member")
        responses.online = true
        store.prepare(serverURL: "test")
        await store.refresh()
        XCTAssertEqual(sends, 0)
        XCTAssertFalse(try XCTUnwrap(store.items.first).isRead)
    }

    func testInitialFailureIsNotAnEmptyInboxAndRefreshKeepsExistingContent() async throws {
        let responses = Responses()
        responses.unavailable = true
        let store = InboxStore(context: context(), defaults: try preferences(),
            fetchPage: { _ in
                if responses.unavailable { throw ServerClientError.response(status: 500, message: "SECRET") }
                return (.init(items: [self.notification()], nextCursor: nil), false)
            }, fetchLocal: { self.snapshot() }, updateReceipt: { _, _ in })
        store.prepare(serverURL: "test")
        await store.refresh()
        XCTAssertTrue(store.hasLoaded)
        XCTAssertEqual(store.errorMessage, ClientFailure.service.message)
        responses.unavailable = false
        await store.refresh()
        XCTAssertNil(store.errorMessage)
        responses.unavailable = true
        await store.refresh()
        XCTAssertEqual(store.items.count, 1)
        XCTAssertNil(store.errorMessage, "A background failure should not duplicate the window's connection state.")
    }

}
