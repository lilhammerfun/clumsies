import XCTest
@testable import Clumsies

@MainActor
final class RecallLoadingTests: XCTestCase {
    func testProjectChangesIgnoreLateResultsAndDuplicateLoads() async throws {
        let firstStarted = expectation(description: "First project requested")
        let secondStarted = expectation(description: "Second project requested")
        var pending: [String: CheckedContinuation<ListRecallsResponse, Error>] = [:]
        var requests: [String] = []
        let model = RecallModel(daemon: DaemonXPCClient(serviceName: "test.unused")) { projectId in
            let id = projectId ?? "all"
            requests.append(id)
            return try await withCheckedThrowingContinuation { continuation in
                pending[id] = continuation
                (id == "first" ? firstStarted : secondStarted).fulfill()
            }
        }
        let first = Task { await model.selectProject("first") }
        await fulfillment(of: [firstStarted], timeout: 1)
        await model.load()
        XCTAssertEqual(requests, ["first"])

        let second = Task { await model.selectProject("second") }
        await fulfillment(of: [secondStarted], timeout: 1)
        try XCTUnwrap(pending.removeValue(forKey: "first"))
            .resume(throwing: URLError(.timedOut))
        await first.value
        XCTAssertTrue(model.isLoading)
        XCTAssertNil(model.errorMessage)
        XCTAssertTrue(model.sessions.isEmpty)

        try XCTUnwrap(pending.removeValue(forKey: "second"))
            .resume(returning: .init(sessions: [], workspaceRoots: []))
        await second.value
        XCTAssertFalse(model.isLoading)
        XCTAssertTrue(model.hasLoaded)
        XCTAssertEqual(model.selectedProjectId, "second")
    }

    func testFailedActivityRefreshRetainsContentAndCanRetry() async {
        var attempt = 0
        let session = RecallSession(host: .codex, sessionId: "test", title: "Test", workspaceRoot: "/test", createdAt: nil, tasks: [])
        let model = RecallModel(daemon: DaemonXPCClient(serviceName: "test.unused")) { _ in
            attempt += 1
            if attempt == 2 { throw URLError(.timedOut) }
            return .init(sessions: [session], workspaceRoots: [])
        }
        await model.load()
        await model.load()
        XCTAssertEqual(model.sessions.map(\.id), [session.id])
        XCTAssertNotNil(model.errorMessage)
        XCTAssertFalse(model.isLoading)
        await model.load()
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(attempt, 3)
    }
}
