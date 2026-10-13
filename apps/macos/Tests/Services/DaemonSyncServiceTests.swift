import XCTest
@testable import Clumsies

@MainActor
final class DaemonSyncServiceTests: XCTestCase {
    func testIdentityAndProjectChangesReleaseBusyStateAndRejectOldSuccessFailureAndDefer() async throws {
        for changesIdentity in [true, false] {
            let context = WorkspaceContext()
            context.phase = .ready
            context.activeProjectId = "first"
            let feedback = WorkspaceFeedback(context: context)
            let oldStarted = expectation(description: "Old refresh")
            let newStarted = expectation(description: "Replacement refresh")
            var pending: [CheckedContinuation<DaemonSyncStatus, Error>] = []
            let service = DaemonSyncService(context: context, feedback: feedback) { _ in
                try await withCheckedThrowingContinuation {
                    pending.append($0)
                    if pending.count == 1 { oldStarted.fulfill() } else { newStarted.fulfill() }
                }
            }
            service.applyRuntime(runtime())
            service.isRefreshingSynchronizedWorkspaceData = true
            let priorRequest = service.synchronizedWorkspaceDataGeneration
            let old = Task { await service.refreshSyncStatus() }
            await fulfillment(of: [oldStarted], timeout: 1)
            XCTAssertTrue(service.isRefreshingSyncStatus)

            if changesIdentity {
                service.resetAuthority()
                context.resetAuthority()
                context.phase = .ready
                context.activeProjectId = "first"
                service.applyRuntime(runtime())
            } else {
                context.activeProjectId = "second"
                context.activeProjectId = "first"
            }
            XCTAssertFalse(service.isRefreshingSyncStatus)
            XCTAssertFalse(service.isRefreshingSynchronizedWorkspaceData)
            XCTAssertNotEqual(service.synchronizedWorkspaceDataGeneration, priorRequest)
            let current = Task { await service.refreshSyncStatus() }
            await fulfillment(of: [newStarted], timeout: 1)
            service.syncStatusAvailable = true
            if changesIdentity { pending[0].resume(returning: status(pending: 99)) }
            else { pending[0].resume(throwing: URLError(.notConnectedToInternet)) }
            await old.value
            XCTAssertNil(service.runtime?.sync, "Old success must not restore the previous account's state.")
            XCTAssertTrue(service.syncStatusAvailable, "An old failure must not replace current connection state.")
            XCTAssertTrue(service.isRefreshingSyncStatus, "Old defer must not release the replacement's gate.")
            await service.refreshSyncStatus()
            XCTAssertEqual(pending.count, 2, "Only one request belongs to the current context.")

            pending[1].resume(returning: status(pending: 1))
            await current.value
            XCTAssertEqual(service.runtime?.sync?.pendingOperationCount, 1)
            XCTAssertTrue(service.syncStatusAvailable)
            XCTAssertFalse(service.isRefreshingSyncStatus)
            service.resetAuthority()
        }
    }

    private func status(pending: Int) -> DaemonSyncStatus {
        let channel = DaemonSyncChannelStatus(state: "ready", serverCursor: nil,
            lastAttemptAt: nil, lastSuccessAt: nil, lastError: nil)
        return .init(draftSync: channel, commitSync: channel, pendingOperationCount: pending,
            failedOperationCount: 0, behindDraftCount: 0, reconciliationConflictCount: 0, lastSuccessAt: nil)
    }

    private func runtime() -> RuntimeState {
        .init(health: .init(daemonVersion: "test", agentRuntime: .init(protocolRevision: 1, buildId: "test"),
            serverUrl: "https://example.test", projectId: "first", daemonInstallationId: "unused",
            logDir: "/unused", localDb: .init(path: "/unused", ready: true, schemaVersion: 1)),
            sync: nil, serverDataSource: "live")
    }
}
