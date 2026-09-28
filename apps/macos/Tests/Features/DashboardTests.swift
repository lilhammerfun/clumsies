import XCTest
@testable import Clumsies

@MainActor
final class DashboardTests: XCTestCase {
    private func snapshot(project: String = "p") -> DashboardSnapshot {
        .init(projectId: project, projectName: project, period: 7,
            memory: .init(generatedAt: 1_789_900_000, dayBounds: [], recencyStarts: [], projectIds: [project],
                resources: [], memoryCount: 244, addedCount: 21, updatedCount: 13, deletedCount: 2,
                days: [], changeBuckets: [], openDrafts: 3, submittedDrafts: 1),
            retrieval: .init(retrievals: 98, recalledCount: 10, coverage: 0.25, days: [],
                directories: [], topResources: [], recency: [], historyStart: nil, retentionPerProject: 500))
    }

    func testPresentationUsesProducerCountsAndUnixTimestamps() throws {
        let original = snapshot()
        let decoded = try JSONCoding.decoder().decode(DashboardSnapshot.self, from: JSONCoding.encoder().encode(original))
        let summary = DashboardSummary(snapshot: decoded)
        XCTAssertEqual(summary.memoryCount, 244, "Use the server count even when no document list is loaded.")
        XCTAssertEqual(summary.changedCount(.updated), 13)
        XCTAssertEqual(summary.retrievals, 98)
        XCTAssertEqual(summary.recalledCount, 10)
        XCTAssertEqual(summary.coverage, 0.25)
        XCTAssertEqual(decoded.generatedAt.timeIntervalSince1970, 1_789_900_000)
    }

    func testDeltaAndRunRatiosUseCountsAndKeepMissingDataUnknown() throws {
        var original = snapshot()
        var retrieval = original.retrieval
        retrieval.delta = .init(withState: 2, days: [
            .init(date: 0, added: 1, replaced: 1, reused: 8, unknown: 2),
            .init(date: 86400, added: 1, replaced: 0, reused: 0, unknown: 0)
        ])
        retrieval.agentUsage = .init(days: [
            .init(date: 0, withMemory: 1, withoutMemory: 9),
            .init(date: 86400, withMemory: 1, withoutMemory: 0)
        ], unreadableSessions: 1, unsupportedSessions: 2)
        original = .init(projectId: original.projectId, projectName: original.projectName,
            period: original.period, memory: original.memory, retrieval: retrieval)
        let decoded = try JSONCoding.decoder().decode(DashboardSnapshot.self, from: JSONCoding.encoder().encode(original))
        XCTAssertEqual(try XCTUnwrap(decoded.retrieval.delta?.reuseRate), 8.0 / 11.0, accuracy: 0.0001)
        XCTAssertEqual(try XCTUnwrap(decoded.retrieval.agentUsage?.usageRate), 2.0 / 11.0, accuracy: 0.0001)
        XCTAssertNil(DashboardDeltaStatistics(withState: 0, days: []).reuseRate)
        XCTAssertNil(DashboardAgentUsage(days: [], unreadableSessions: 1, unsupportedSessions: 1).usageRate)
        XCTAssertNil(snapshot().retrieval.delta, "Older runtimes must show unavailable, not zero reuse.")
    }

    func testSwitchingProjectRejectsLateResults() async throws {
        let model = DashboardModel()
        let started = expectation(description: "First request started")
        var resume: CheckedContinuation<(DashboardSnapshot, Bool), Error>?
        let old = Task {
            await model.load(key: "old") {
                try await withCheckedThrowingContinuation { resume = $0; started.fulfill() }
            }
        }
        await fulfillment(of: [started], timeout: 1)
        await model.load(key: "new") { (self.snapshot(project: "new"), false) }
        try XCTUnwrap(resume).resume(returning: (snapshot(project: "old"), true))
        await old.value
        XCTAssertEqual(model.snapshot?.projectID, "new")
        XCTAssertFalse(model.isDemo)
        XCTAssertFalse(model.isLoading)
    }

    func testFailedRefreshRetainsSameScopeAndSwitchClearsIt() async {
        let model = DashboardModel()
        await model.load(key: "p") { (self.snapshot(), true) }
        await model.load(key: "p") { throw URLError(.timedOut) }
        XCTAssertEqual(model.snapshot?.projectID, "p")
        XCTAssertTrue(model.isShowingSavedContent)
        XCTAssertNil(model.errorMessage, "Retained content must not repeat a connection error.")
        await model.load(key: "other") { throw URLError(.timedOut) }
        XCTAssertNil(model.snapshot)
        XCTAssertFalse(model.isShowingSavedContent)
        XCTAssertEqual(model.errorMessage, ClientFailure.connection.message)
        XCTAssertFalse(model.isDemo)
    }

    func testSuccessfulRefreshClearsRetainedContentNotice() async {
        let model = DashboardModel()
        await model.load(key: "p") { (self.snapshot(), false) }
        await model.load(key: "p") { throw ServerClientError.response(status: 403, message: "private") }
        XCTAssertTrue(model.isShowingSavedContent)
        XCTAssertEqual(model.errorMessage, ClientFailure.forbidden.message)
        await model.load(key: "p") { (self.snapshot(), false) }
        XCTAssertFalse(model.isShowingSavedContent)
        XCTAssertNil(model.errorMessage)
    }

}
