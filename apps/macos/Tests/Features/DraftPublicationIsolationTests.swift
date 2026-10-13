import Foundation
import XCTest
@testable import Clumsies

@MainActor
final class DraftPublicationIsolationTests: XCTestCase {
    func testAcknowledgedGuidelinesCannotPublishAfterContextChangesDuringBaseRetention() async throws {
        for change in [Change.project, .authorityReset, .authorityOnly, .reload, .loading] {
            let started = expectation(description: "Guideline Base retention started")
            let probe = PublicationBaseProbe(started: started)
            let workspace = workspace(probe)
            defer { workspace.clearAuthorityScopedWorkspace() }
            let creation = Task {
                try await workspace.memory.createMemoryGuidelines(.init(
                    projectId: "project", path: MemoryGuidelines.defaultPath, action: .createDefault
                ))
            }
            await fulfillment(of: [started], timeout: 2)
            let acknowledged = await probe.acknowledgedDraftIds
            XCTAssertFalse(acknowledged.isEmpty, "The daemon has already acknowledged this batch before retaining its Base.")
            change.apply(to: workspace)
            workspace.navigation.selectedItemId = "current-selection"
            await probe.release()
            do { _ = try await creation.value; XCTFail("A changed context must reject the late UI result.") }
            catch is CancellationError { }
            XCTAssertTrue(workspace.edits.drafts.isEmpty)
            XCTAssertEqual(workspace.navigation.selectedItemId, "current-selection")
            let stillAcknowledged = await probe.acknowledgedDraftIds
            XCTAssertEqual(stillAcknowledged, acknowledged, "Discarding a late UI result must not undo acknowledged drafts.")
        }
    }

    func testAcknowledgedGuidelinesPublishWhenTheirContextRemainsCurrent() async throws {
        let started = expectation(description: "Guideline Base retention started")
        let probe = PublicationBaseProbe(started: started)
        let workspace = workspace(probe)
        defer { workspace.clearAuthorityScopedWorkspace() }
        let creation = Task {
            try await workspace.memory.createMemoryGuidelines(.init(
                projectId: "project", path: MemoryGuidelines.defaultPath, action: .createDefault
            ))
        }
        await fulfillment(of: [started], timeout: 2)
        await probe.release()
        let guidelineId = try await creation.value
        let acknowledged = await probe.acknowledgedDraftIds
        XCTAssertEqual(guidelineId, acknowledged.first)
        XCTAssertEqual(workspace.edits.drafts.count, acknowledged.count)
        XCTAssertEqual(workspace.navigation.selectedItemId, guidelineId)
        XCTAssertTrue(workspace.edits.drafts.allSatisfy { $0.baseCommitPin?.commitId == "base" })
    }

    func testLateSingleDraftRefreshCannotRepopulateChangedAuthorityOrReload() async throws {
        for change in [Change.authorityReset, .authorityOnly, .reload, .loading] {
            let started = expectation(description: "Draft refresh Base retention started")
            let probe = PublicationBaseProbe(started: started)
            let workspace = workspace(probe)
            defer { workspace.clearAuthorityScopedWorkspace() }
            let refresh = Task { try await workspace.edits.refreshDraft("draft") }
            await fulfillment(of: [started], timeout: 2)
            change.apply(to: workspace)
            await probe.release()
            do { try await refresh.value; XCTFail("Late details cannot repopulate another workspace generation.") }
            catch is CancellationError { }
            XCTAssertTrue(workspace.edits.drafts.isEmpty)
        }
    }

    func testReconciliationRejectsAContextResetWhileItsBaseIsRetained() async throws {
        let started = expectation(description: "Reconciliation Base retention started")
        let probe = PublicationBaseProbe(started: started)
        let workspace = workspace(probe)
        defer { workspace.clearAuthorityScopedWorkspace() }
        let draft = WorkspaceLoader.mapDraft(try await probe.detail("draft"), resources: [])
        let synchronization = Task {
            try await workspace.reconciliation.synchronizedDraftForReconciliation(itemId: "draft", draft: draft)
        }
        await fulfillment(of: [started], timeout: 2)
        workspace.clearAuthorityScopedWorkspace()
        await probe.release()
        do { _ = try await synchronization.value; XCTFail("An old reconciliation result cannot continue into a new authority.") }
        catch is CancellationError { }
        XCTAssertTrue(workspace.edits.drafts.isEmpty)
    }

    func testLateReconciliationReadsCannotRetryOrPropagateErrorsAfterContextChanges() async throws {
        for readStatus in [DaemonDraftSyncState.queued, .failed, .synced] {
            for change in [Change.authorityReset, .authorityOnly, .reload, .loading] {
                let started = expectation(description: "Reconciliation draft read started")
                let detail = try PublicationBaseProbe.fixture("draft", operation: .create(
                    path: "draft.md", content: .init(description: nil, content: "Acknowledged text"), description: nil
                ), syncStatus: readStatus)
                let probe = ReconciliationReadProbe(started: started, detail: detail, failsRead: readStatus == .synced)
                let workspace = WorkspaceCoordinator()
                workspace.context.phase = .ready
                workspace.context.activeProjectId = "project"
                defer { workspace.clearAuthorityScopedWorkspace() }
                var retryProjects: [String] = []
                let reconciliation = DraftReconciliationService(
                    catalog: workspace.catalog, context: workspace.context, edits: workspace.edits,
                    refresh: workspace.refresh, sessions: workspace.sessions,
                    loadDraft: { try await probe.read($0) },
                    retryDraftSync: { retryProjects.append($0); return .failed("Old retry failure") }
                )
                let draft = WorkspaceLoader.mapDraft(detail, resources: [])
                let synchronization = Task {
                    try await reconciliation.synchronizedDraftForReconciliation(itemId: "draft", draft: draft)
                }
                await fulfillment(of: [started], timeout: 2)
                change.apply(to: workspace)
                await probe.release()
                do { _ = try await synchronization.value; XCTFail("A stale read must not continue reconciliation.") }
                catch is CancellationError { }
                XCTAssertTrue(retryProjects.isEmpty, "A stale wait result must not synchronize the old project in the new context.")
                XCTAssertTrue(workspace.edits.drafts.isEmpty)
                XCTAssertNil(workspace.feedback.errorMessage)
            }
        }
    }

    func testLateReconciliationRetryFailureIsCancelledAfterContextChanges() async throws {
        for change in [Change.authorityReset, .authorityOnly, .reload, .loading] {
            let started = expectation(description: "Reconciliation sync retry started")
            let probe = ReconciliationRetryProbe(started: started)
            let detail = try PublicationBaseProbe.fixture("draft", operation: .create(
                path: "draft.md", content: .init(description: nil, content: "Acknowledged text"), description: nil
            ), syncStatus: .queued)
            let workspace = WorkspaceCoordinator()
            workspace.context.phase = .ready
            workspace.context.activeProjectId = "project"
            defer { workspace.clearAuthorityScopedWorkspace() }
            let reconciliation = DraftReconciliationService(
                catalog: workspace.catalog, context: workspace.context, edits: workspace.edits,
                refresh: workspace.refresh, sessions: workspace.sessions,
                loadDraft: { _ in detail }, retryDraftSync: { await probe.retry($0) }
            )
            let draft = WorkspaceLoader.mapDraft(detail, resources: [])
            let synchronization = Task {
                try await reconciliation.synchronizedDraftForReconciliation(itemId: "draft", draft: draft)
            }
            await fulfillment(of: [started], timeout: 2)
            change.apply(to: workspace)
            await probe.release()
            do { _ = try await synchronization.value; XCTFail("An old retry failure must not enter the changed context.") }
            catch is CancellationError { }
            let retryProjects = await probe.projects
            XCTAssertEqual(retryProjects, ["project"], "The already-started retry is not reversed or repeated.")
            XCTAssertNil(workspace.feedback.errorMessage)
        }
    }

    private func workspace(_ probe: PublicationBaseProbe) -> WorkspaceCoordinator {
        let workspace = WorkspaceCoordinator(
            loadDraft: { try await probe.detail($0) },
            createMemoryDrafts: { try await probe.create($0) },
            retainBase: { await probe.retain($0) }
        )
        workspace.context.phase = .ready
        workspace.context.activeProjectId = "project"
        workspace.context.projects = ["project", "other"].map {
            ProjectState(id: $0, name: $0, refCommitId: "base", refEtag: "etag",
                         selectedOrgResourceIds: [], orgSelectionRevision: 1, isLoaded: true)
        }
        return workspace
    }

    private enum Change {
        case project, authorityReset, authorityOnly, reload, loading

        @MainActor
        func apply(to workspace: WorkspaceCoordinator) {
            switch self {
            case .project: workspace.context.activeProjectId = "other"
            case .authorityReset: workspace.clearAuthorityScopedWorkspace()
            case .authorityOnly: workspace.context.invalidateAdministrationAuthority()
            case .reload: workspace.context.workspaceReloadGeneration = UUID()
            case .loading: workspace.context.phase = .loading
            }
        }
    }
}

private actor PublicationBaseProbe {
    let started: XCTestExpectation
    private var pending: CheckedContinuation<CommitBasePin, Never>?
    private var details: [String: DaemonDraftDetail] = [:]
    private(set) var acknowledgedDraftIds: [String] = []

    init(started: XCTestExpectation) { self.started = started }

    func retain(_ commitId: String) async -> CommitBasePin {
        await withCheckedContinuation { pending = $0; started.fulfill() }
    }

    func release() {
        pending?.resume(returning: CommitBasePin(commitId: "base") { })
        pending = nil
    }

    func create(_ request: DaemonCreateMemoryDraftsRequest) throws -> [DaemonDraftOperationResponse] {
        try request.operations.enumerated().map { index, operation in
            let id = "created-\(index)"
            details[id] = try Self.fixture(id, operation: operation)
            acknowledgedDraftIds.append(id)
            return .init(localOperationId: "op-\(index)", draftId: id, queued: false, syncStatus: .synced)
        }
    }

    func detail(_ id: String) throws -> DaemonDraftDetail {
        if let detail = details[id] { return detail }
        return try Self.fixture(id, operation: .create(
            path: "draft.md", content: .init(description: nil, content: "Acknowledged text"), description: nil
        ))
    }

    nonisolated static func fixture(_ id: String, operation: DaemonDraftOperation,
                                    syncStatus: DaemonDraftSyncState = .synced) throws -> DaemonDraftDetail {
        guard case .create(let path, _, _) = operation else { throw FixtureError.unexpectedOperation }
        let operationJSON = try JSONSerialization.jsonObject(with: JSONCoding.encoder().encode(operation))
        let json: [String: Any] = [
            "draft": [
                "draft_id": id, "project_id": "project",
                "server_draft_id": syncStatus == .synced ? "remote-\(id)" as Any : NSNull(),
                "server_version": 1, "base_commit_id": "base", "current_commit_id": "base",
                "freshness": "current", "has_upstream_resource_changes": false,
                "reconciliation": "unknown", "reconciliation_candidate_id": NSNull(),
                "scope": "project", "resource_kind": "memory", "target_id": NSNull(),
                "path": path, "status": "open", "created_at": "now", "updated_at": "now",
                "pending_operation_count": syncStatus == .queued ? 1 : 0,
                "failed_operation_count": syncStatus == .failed ? 1 : 0,
            ],
            "operations": [[
                "local_operation_id": "op-\(id)", "resource_kind": "memory", "operation": operationJSON,
                "source": "desktop", "sync_status": syncStatus.rawValue,
                "last_error": syncStatus == .failed ? "Old upload failure" as Any : NSNull(),
                "created_at": "now", "updated_at": "now",
            ]],
        ]
        return try JSONCoding.decoder().decode(DaemonDraftDetail.self, from: JSONSerialization.data(withJSONObject: json))
    }

    private enum FixtureError: Error { case unexpectedOperation }
}

private actor ReconciliationReadProbe {
    let started: XCTestExpectation
    let detail: DaemonDraftDetail
    let failsRead: Bool
    private var pending: CheckedContinuation<DaemonDraftDetail, Error>?

    init(started: XCTestExpectation, detail: DaemonDraftDetail, failsRead: Bool) {
        self.started = started
        self.detail = detail
        self.failsRead = failsRead
    }

    func read(_ id: String) async throws -> DaemonDraftDetail {
        try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
    }

    func release() {
        if failsRead { pending?.resume(throwing: ReconciliationReadError.oldReadFailed) }
        else { pending?.resume(returning: detail) }
        pending = nil
    }

    private enum ReconciliationReadError: Error { case oldReadFailed }
}

private actor ReconciliationRetryProbe {
    let started: XCTestExpectation
    private var pending: CheckedContinuation<SyncRetryOutcome, Never>?
    private(set) var projects: [String] = []

    init(started: XCTestExpectation) { self.started = started }

    func retry(_ projectId: String) async -> SyncRetryOutcome {
        projects.append(projectId)
        return await withCheckedContinuation { pending = $0; started.fulfill() }
    }

    func release() {
        pending?.resume(returning: .failed("Old retry failure"))
        pending = nil
    }
}
