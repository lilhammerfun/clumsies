import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class ActivityLoadingTests: XCTestCase {
    private func summary(_ id: String) -> RecallSessionSummary {
        .init(host: .codex, sessionId: id, title: id, workspaceRoot: "/test", createdAt: nil, sessionToken: id)
    }

    private func session(_ id: String, tasks: [String] = []) -> RecallSession {
        .init(host: .codex, sessionId: id, title: id, workspaceRoot: "/test", createdAt: nil,
              tasks: tasks.map { .init(messageId: $0, text: $0, time: nil, activations: []) })
    }

    func testProjectChangesIgnoreLateResultsAndDuplicateLoads() async throws {
        let firstStarted = expectation(description: "First project requested")
        let secondStarted = expectation(description: "Second project requested")
        var pending: [String: CheckedContinuation<ListRecallsResponse, Error>] = [:]
        var requests: [String] = []
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.unused"), fetchSessions: { request in
            let id = request.projectId ?? "all"
            requests.append(id)
            return try await withCheckedThrowingContinuation { continuation in
                pending[id] = continuation
                (id == "first" ? firstStarted : secondStarted).fulfill()
            }
        })
        let first = Task { await model.selectProject("first") }
        await fulfillment(of: [firstStarted], timeout: 1)
        await model.load()
        XCTAssertEqual(requests, ["first"])
        let second = Task { await model.selectProject("second") }
        await fulfillment(of: [secondStarted], timeout: 1)
        try XCTUnwrap(pending.removeValue(forKey: "first")).resume(throwing: URLError(.timedOut))
        await first.value
        XCTAssertTrue(model.isLoading)
        XCTAssertNil(model.errorMessage)
        XCTAssertTrue(model.sessions.isEmpty)
        try XCTUnwrap(pending.removeValue(forKey: "second")).resume(returning: .init(sessions: [], workspaceRoots: []))
        await second.value
        XCTAssertFalse(model.isLoading)
        XCTAssertTrue(model.hasLoaded)
        XCTAssertEqual(model.selectedProjectId, "second")
    }

    func testFailedActivityRefreshRetainsContentAndCanRetry() async {
        var attempt = 0
        let row = summary("test")
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.unused"), fetchSessions: { _ in
            attempt += 1
            if attempt == 2 { throw URLError(.timedOut) }
            return .init(sessions: [row], workspaceRoots: [])
        })
        await model.load()
        await model.load()
        XCTAssertEqual(model.sessions.map(\.id), [row.id])
        XCTAssertNotNil(model.errorMessage)
        XCTAssertFalse(model.isLoading)
        await model.load()
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(attempt, 3)
    }

    func testListPaginationAndSelectedDetailsAreIndependent() async {
        let first = summary("first"), second = summary("second")
        var listCursors: [String?] = []
        var detailRequests: [GetRecallSessionRequest] = []
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.unused"), fetchSessions: { request in
            listCursors.append(request.cursor)
            return request.cursor == nil
                ? .init(sessions: [first], workspaceRoots: [], nextCursor: "next")
                : .init(sessions: [second], workspaceRoots: [])
        }, fetchDetail: { request in
            detailRequests.append(request)
            return .init(session: self.session("first", tasks: [request.offset == nil ? "one" : "two"]),
                         totalTasks: 2, nextOffset: request.offset == nil ? 1 : nil)
        })
        await model.load()
        XCTAssertTrue(detailRequests.isEmpty, "A list request must not fetch session bodies.")
        await model.loadMoreSessions()
        await model.loadMoreSessions()
        XCTAssertEqual(listCursors, [nil, "next"])
        XCTAssertEqual(model.sessions.map(\.id), [first.id, second.id])
        XCTAssertEqual(model.selectedSessionId, first.id)
        await model.loadSelectedSession()
        XCTAssertEqual(model.selectedSession?.tasks.count, 1)
        await model.loadMoreTasks()
        XCTAssertEqual(model.selectedSession?.tasks.map(\.id), ["one", "two"])
        XCTAssertEqual(detailRequests.map(\.sessionToken), ["first", "first"])
        XCTAssertEqual(detailRequests.map(\.offset), [nil, 1])
        XCTAssertNil(model.nextTaskOffset)
    }

    func testLateDetailCannotReplaceNewSelection() async throws {
        let started = expectation(description: "Old detail started")
        var pending: CheckedContinuation<GetRecallSessionResponse, Error>?
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.unused"), fetchSessions: { _ in
            .init(sessions: [self.summary("first"), self.summary("second")], workspaceRoots: [])
        }, fetchDetail: { request in
            if request.sessionToken == "first" {
                return try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
            }
            return .init(session: self.session("second"), totalTasks: 0, nextOffset: nil)
        })
        await model.load()
        let old = Task { await model.loadSelectedSession() }
        await fulfillment(of: [started], timeout: 1)
        model.selectedSessionId = "codex:second"
        await model.loadSelectedSession()
        try XCTUnwrap(pending).resume(returning: .init(session: session("first"), totalTasks: 0, nextOffset: nil))
        await old.value
        XCTAssertEqual(model.selectedSession?.sessionId, "second")
        XCTAssertFalse(model.isLoadingDetail)
    }

    func testFailedNextPageKeepsRowsAndCursorForRetry() async {
        var pageAttempts = 0
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.unused"), fetchSessions: { request in
            if request.cursor == nil { return .init(sessions: [self.summary("first")], workspaceRoots: [], nextCursor: "next") }
            pageAttempts += 1
            if pageAttempts == 1 { throw URLError(.timedOut) }
            return .init(sessions: [self.summary("second")], workspaceRoots: [])
        })
        await model.load()
        await model.loadMoreSessions()
        XCTAssertEqual(model.sessions.count, 1)
        XCTAssertEqual(model.nextCursor, "next")
        XCTAssertNotNil(model.pageError)
        await model.loadMoreSessions()
        XCTAssertEqual(model.sessions.count, 2)
        XCTAssertNil(model.pageError)
    }

    func testListLoadsNextPageOnlyWhenItsFooterBecomesVisible() async throws {
        let nextRequested = expectation(description: "Next page requested after scrolling")
        var requests = 0
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.unused"), fetchSessions: { request in
            requests += 1
            if request.cursor != nil {
                nextRequested.fulfill()
                return .init(sessions: [self.summary("next")], workspaceRoots: [])
            }
            return .init(sessions: (0..<40).map { self.summary("row-\($0)") }, workspaceRoots: [], nextCursor: "next")
        })
        await model.load()
        let hosting = NSHostingView(rootView: ActivitySessionList(model: model))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 320, height: 400),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = hosting
        window.orderFront(nil)
        defer { window.close() }
        hosting.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(250))
        XCTAssertEqual(requests, 1, "Offscreen pagination must not eagerly drain all pages.")
        func scrollView(in view: NSView) -> NSScrollView? {
            if let scroll = view as? NSScrollView { return scroll }
            return view.subviews.lazy.compactMap { scrollView(in: $0) }.first
        }
        let scroll = try XCTUnwrap(scrollView(in: hosting))
        let document = try XCTUnwrap(scroll.documentView)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, document.bounds.height - scroll.contentView.bounds.height)))
        scroll.reflectScrolledClipView(scroll.contentView)
        await fulfillment(of: [nextRequested], timeout: 2)
        XCTAssertEqual(requests, 2)
    }

    func testProjectSwitchDiscardsAnInFlightPage() async throws {
        let started = expectation(description: "Continuation started")
        var pending: CheckedContinuation<ListRecallsResponse, Error>?
        var pages = 0
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.unused"), fetchSessions: { request in
            if request.cursor != nil {
                pages += 1
                return try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
            }
            return .init(sessions: [self.summary(request.projectId ?? "all")], workspaceRoots: [], nextCursor: "next")
        })
        await model.selectProject("first")
        let old = Task { await model.loadMoreSessions() }
        await fulfillment(of: [started], timeout: 1)
        await model.loadMoreSessions()
        XCTAssertEqual(pages, 1)
        await model.selectProject("second")
        try XCTUnwrap(pending).resume(returning: .init(sessions: [summary("old-page")], workspaceRoots: []))
        await old.value
        XCTAssertEqual(model.sessions.map(\.sessionId), ["second"])
        XCTAssertEqual(model.nextCursor, "next")
        XCTAssertFalse(model.isLoadingMore)
    }

    func testRefreshPreservesLoadedTaskPagesAndFailureKeepsDetail() async {
        var detailCalls = 0
        let model = ActivityModel(daemon: DaemonXPCClient(serviceName: "test.unused"), fetchSessions: { _ in
            .init(sessions: [self.summary("first")], workspaceRoots: [])
        }, fetchDetail: { request in
            detailCalls += 1
            if detailCalls == 5 { throw URLError(.timedOut) }
            return .init(session: self.session("first", tasks: [request.offset == nil ? "one" : "two"]),
                         totalTasks: 2, nextOffset: request.offset == nil ? 1 : nil)
        })
        await model.load()
        await model.loadSelectedSession()
        await model.loadMoreTasks()
        await model.loadSelectedSession()
        XCTAssertEqual(detailCalls, 4)
        XCTAssertEqual(model.selectedSession?.tasks.map(\.id), ["one", "two"])
        await model.loadSelectedSession()
        XCTAssertNotNil(model.detailError)
        XCTAssertEqual(model.selectedSession?.tasks.map(\.id), ["one", "two"])
    }

    func testRestoresLastProjectBeforeLoadingAndFallsBackWhenUnavailable() async throws {
        let suite = "ActivityLoadingTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var requests: [String?] = []
        func makeModel() -> ActivityModel {
            ActivityModel(daemon: DaemonXPCClient(serviceName: "test.unused"), defaults: defaults, fetchSessions: { request in
                requests.append(request.projectId)
                return .init(sessions: [], workspaceRoots: [])
            })
        }
        let first = makeModel()
        first.prepare(projectIds: ["a", "b"], preferredProjectId: "b", scope: "org")
        await first.load()
        XCTAssertEqual(requests, ["b"])
        await first.selectProject("a")
        let reopened = makeModel()
        reopened.prepare(projectIds: ["a", "b"], preferredProjectId: "b", scope: "org")
        XCTAssertEqual(reopened.selectedProjectId, "a")
        await reopened.selectProject(nil)
        reopened.prepare(projectIds: ["a", "b"], preferredProjectId: "b", scope: "org")
        XCTAssertNil(reopened.selectedProjectId, "Explicit All Projects remains selected in this session.")
        let afterAll = makeModel()
        afterAll.prepare(projectIds: ["a", "b"], preferredProjectId: "b", scope: "org")
        XCTAssertEqual(afterAll.selectedProjectId, "a")
        afterAll.prepare(projectIds: ["b"], preferredProjectId: "b", scope: "org")
        XCTAssertEqual(afterAll.selectedProjectId, "b")
        afterAll.prepare(projectIds: ["a", "b"], preferredProjectId: "a", scope: "other-org")
        XCTAssertEqual(afterAll.selectedProjectId, "a")
        let empty = makeModel()
        empty.prepare(projectIds: [], preferredProjectId: nil, scope: "empty")
        XCTAssertTrue(empty.hasLoaded)
        empty.prepare(projectIds: ["new"], preferredProjectId: nil, scope: "empty")
        XCTAssertEqual(empty.selectedProjectId, "new")
        XCTAssertFalse(empty.hasLoaded)
    }
}
