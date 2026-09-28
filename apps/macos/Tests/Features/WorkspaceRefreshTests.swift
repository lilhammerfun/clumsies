import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class WorkspaceRefreshTests: XCTestCase {
    func testRefreshFooterIsHiddenDuringNormalPollingAndStableWhileRetryingFailure() async throws {
        let scheduler = WorkspaceRefreshScheduler()
        let response = Response()
        scheduler.register(.memory) { response.available ? .updated : .retained }

        func footerHeight() -> CGFloat {
            let host = NSHostingView(rootView: WorkspaceRefreshStatusView(scheduler: scheduler).frame(width: 600))
            return host.fittingSize.height
        }

        XCTAssertEqual(footerHeight(), 0, "An unchecked page must not reserve a status bar")
        let first = try XCTUnwrap(scheduler.request(.memory))
        XCTAssertEqual(scheduler.statuses[.memory]?.isRefreshing, true)
        XCTAssertEqual(footerHeight(), 0, "Background polling must not flash a loading bar")
        await first.value
        XCTAssertEqual(footerHeight(), 0, "Successful checks must not show a ticking timestamp")

        response.available = false
        await scheduler.request(.memory)?.value
        let warningHeight = footerHeight()
        XCTAssertGreaterThan(warningHeight, 0, "Retained data must still have a recovery prompt")
        response.available = true
        let retry = try XCTUnwrap(scheduler.request(.memory))
        XCTAssertEqual(footerHeight(), warningHeight, "Keep the warning in place while retrying")
        await retry.value
        XCTAssertEqual(footerHeight(), 0, "Recovery restores the full content area")
    }

    func testCancellationBeforeExecutionDoesNotStartLoader() async {
        let scheduler = WorkspaceRefreshScheduler()
        var calls = 0
        scheduler.register(.memory) { calls += 1; return .updated }
        let task = scheduler.request(.memory)
        scheduler.cancel()
        await task?.value
        XCTAssertEqual(calls, 0)
        XCTAssertNil(scheduler.statuses[.memory]?.lastSuccess)
        XCTAssertEqual(scheduler.statuses[.memory]?.isRefreshing, false)
    }

    func testBackgroundTransitionDuringStartupIsRememberedWithoutStartingReads() {
        let workspace = WorkspaceCoordinator()
        workspace.refreshVisiblePage(isForeground: false)
        XCTAssertFalse(workspace.refreshes.isForeground)
        XCTAssertFalse(workspace.refreshes.statuses.values.contains { $0.isRefreshing })
        XCTAssertEqual(workspace.refreshes.interval(for: .memory), 60)
    }

    func testSlowMemoryDoesNotBlockInboxOrSyncAndBurstsCoalesce() async throws {
        let scheduler = WorkspaceRefreshScheduler()
        let started = expectation(description: "Memory blocked")
        let followUp = expectation(description: "One trailing refresh")
        var continuation: CheckedContinuation<WorkspaceRefreshScheduler.Result, Never>?
        var reads = 0
        scheduler.register(.memory) {
            reads += 1
            if reads == 1 {
                return await withCheckedContinuation { continuation = $0; started.fulfill() }
            }
            followUp.fulfill()
            return .updated
        }
        scheduler.register(.inbox) { .updated }
        scheduler.register(.sync) { .updated }
        let memory = try XCTUnwrap(scheduler.request(.memory))
        await fulfillment(of: [started], timeout: 1)
        await scheduler.request(.inbox)?.value
        await scheduler.request(.sync)?.value
        XCTAssertNotNil(scheduler.statuses[.inbox]?.lastSuccess)
        XCTAssertNotNil(scheduler.statuses[.sync]?.lastSuccess)
        XCTAssertNil(scheduler.statuses[.memory]?.lastSuccess)
        for _ in 0..<20 { scheduler.invalidate([.memory]) }
        XCTAssertEqual(reads, 1)
        try XCTUnwrap(continuation).resume(returning: .updated)
        await memory.value
        await fulfillment(of: [followUp], timeout: 1)
        XCTAssertEqual(reads, 2)
        scheduler.cancel()
    }

    func testVisibleDeadlinesForegroundRecoveryAndBackgroundRequestBudget() async {
        var date = Date(timeIntervalSince1970: 100)
        let scheduler = WorkspaceRefreshScheduler(now: { date })
        var counts: [WorkspaceRefreshScheduler.Domain: Int] = [:]
        for domain in [WorkspaceRefreshScheduler.Domain.memory, .inbox, .dashboard] {
            scheduler.register(domain) { counts[domain, default: 0] += 1; return .updated }
            await scheduler.request(domain)?.value
        }
        scheduler.visible = .memory
        date += 5
        scheduler.tick()
        await scheduler.request(.memory)?.value
        XCTAssertEqual(counts[.memory], 2)
        XCTAssertEqual(counts[.inbox], 1)
        XCTAssertEqual(counts[.dashboard], 1)
        scheduler.visible = .inbox
        scheduler.tick()
        await scheduler.request(.inbox)?.value
        XCTAssertEqual(counts[.inbox], 2, "An expired page refreshes on entry, without waiting for a background deadline")
        scheduler.isForeground = false
        date += 10
        scheduler.tick()
        XCTAssertEqual(counts[.inbox], 2)
        scheduler.isForeground = true
        scheduler.tick()
        await scheduler.request(.inbox)?.value
        XCTAssertEqual(counts[.inbox], 3)
        XCTAssertEqual(counts[.dashboard], 1, "Foreground activation does not reload unexpired statistics")
        scheduler.cancel()
    }

    func testResetCancelsOldReadWithoutPublishingItsSuccessOrClearingNewLoading() async throws {
        let scheduler = WorkspaceRefreshScheduler()
        let started = expectation(description: "Old read")
        var continuation: CheckedContinuation<WorkspaceRefreshScheduler.Result, Never>?
        scheduler.register(.memory) {
            await withCheckedContinuation { continuation = $0; started.fulfill() }
        }
        let old = try XCTUnwrap(scheduler.request(.memory))
        await fulfillment(of: [started], timeout: 1)
        scheduler.reset()
        scheduler.register(.memory) { .retained }
        await scheduler.request(.memory)?.value
        try XCTUnwrap(continuation).resume(returning: .updated)
        await old.value
        XCTAssertNil(scheduler.statuses[.memory]?.lastSuccess)
        XCTAssertEqual(scheduler.statuses[.memory]?.isStale, true)
        XCTAssertEqual(scheduler.statuses[.memory]?.isRefreshing, false)
    }

    private final class Response {
        var available = true
        var calls = 0
    }

    func testFailedRefreshKeepsLastSuccessUntilRecoveryAndDoesNotSpin() async {
        var date = Date(timeIntervalSince1970: 100)
        let scheduler = WorkspaceRefreshScheduler(now: { date })
        let response = Response()
        scheduler.register(.memory) { response.calls += 1; return response.available ? .updated : .retained }
        await scheduler.request(.memory)?.value
        let lastSuccess = scheduler.statuses[.memory]?.lastSuccess
        response.available = false
        date += 1
        await scheduler.request(.memory)?.value
        XCTAssertEqual(scheduler.statuses[.memory]?.lastSuccess, lastSuccess)
        XCTAssertEqual(scheduler.statuses[.memory]?.isStale, true)
        scheduler.tick()
        XCTAssertEqual(response.calls, 2)
        response.available = true
        date += 1
        await scheduler.request(.memory)?.value
        XCTAssertEqual(scheduler.statuses[.memory]?.lastSuccess, date)
        XCTAssertEqual(scheduler.statuses[.memory]?.isStale, false)
    }

    func testHiddenMutationBurstsWaitForVisibilityInsteadOfReloadingEveryPage() async {
        let scheduler = WorkspaceRefreshScheduler()
        var calls = 0
        scheduler.register(.inbox) { calls += 1; return .updated }
        await scheduler.request(.inbox)?.value
        for _ in 0..<20 { scheduler.invalidate([.inbox]) }
        scheduler.tick()
        XCTAssertEqual(calls, 1)
        scheduler.show(.inbox, isForeground: true)
        scheduler.tick()
        await scheduler.request(.inbox)?.value
        XCTAssertEqual(calls, 2)
        scheduler.cancel()
    }

    func testPageRegistrationCannotRemoveItsReplacementAndSurvivesTimingReset() {
        let scheduler = WorkspaceRefreshScheduler()
        let old = scheduler.register(.dashboard) { .updated }
        let current = scheduler.register(.dashboard) { .updated }
        scheduler.unregister(.dashboard, id: old)
        XCTAssertNotNil(scheduler.statuses[.dashboard])
        scheduler.reset()
        scheduler.unregister(.dashboard, id: current)
        XCTAssertNil(scheduler.statuses[.dashboard])
    }
}
