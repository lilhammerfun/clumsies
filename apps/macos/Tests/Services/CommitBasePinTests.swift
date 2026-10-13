import Darwin
import XCTest
@testable import Clumsies

final class CommitBasePinTests: XCTestCase {
    func testSharedFileLockPrecedesAcknowledgementAndLivesUntilLastCopy() async throws {
        let path = try lockFile()
        let probe = PinRequestProbe(path: path)
        var pin: CommitBasePin? = try await DaemonXPCClient(serviceName: "unused").retainCommitBase("base") {
            try await probe.request($0)
        }
        let acquired = await probe.acknowledgedWhileLocked
        XCTAssertTrue(acquired)
        XCTAssertFalse(try canLockExclusively(path))
        var copy = pin
        pin = nil
        XCTAssertFalse(try canLockExclusively(path))
        XCTAssertEqual(copy?.commitId, "base")
        copy = nil
        XCTAssertTrue(try canLockExclusively(path))
    }

    func testFailedAcknowledgementAndSymlinkCannotPublishOrLeakPin() async throws {
        let path = try lockFile()
        let probe = PinRequestProbe(path: path, rejectsAcknowledgement: true)
        do {
            _ = try await DaemonXPCClient(serviceName: "unused").retainCommitBase("base") {
                try await probe.request($0)
            }
            XCTFail("A failed acknowledgement must never return an editable Base.")
        } catch PinTestError.rejected { }
        XCTAssertTrue(try canLockExclusively(path))
        let link = path + ".symlink"
        try FileManager.default.createSymbolicLink(atPath: link, withDestinationPath: path)
        do {
            _ = try await DaemonXPCClient(serviceName: "unused").retainCommitBase("base") { _ in
                .init(pinId: "pin", commitId: "base", lockPath: link)
            }
            XCTFail("The pin file must not be followed through a symlink.")
        } catch { }
        XCTAssertTrue(try canLockExclusively(path))
    }

    func testExpiredProvisionalPinRetriesOnceAndClosesOldDescriptor() async throws {
        let path = try lockFile()
        let probe = PinRequestProbe(path: path, expiresFirstAcknowledgement: true)
        var pin: CommitBasePin? = try await DaemonXPCClient(serviceName: "unused").retainCommitBase("base") {
            try await probe.request($0)
        }
        let issues = await probe.issues
        XCTAssertEqual(issues, 2)
        XCTAssertEqual(pin?.commitId, "base")
        pin = nil
        XCTAssertTrue(try canLockExclusively(path))
    }

    @MainActor
    func testLazyProjectSnapshotSharesOneAcknowledgedBaseAndPinFailureRejectsSnapshot() async throws {
        for rejects in [false, true] {
            let probe = SnapshotPinProbe(rejects: rejects)
            let daemon = DaemonXPCClient(serviceName: "unused")
            let server = ServerClient(daemon: daemon) { request in
                let body: String
                if request.path.hasSuffix("/commit-state") {
                    body = #"{"update_available":false,"ref":{"name":"project/main","scope":"project","org_id":"org","project_id":"project","commit_id":"base","updated_at":"now"},"latest":null,"download_url":null,"incremental_supported":false}"#
                } else if request.path.hasSuffix("/org-selections") {
                    body = #"{"project_id":"project","memories":[],"revision":1}"#
                } else {
                    body = #"{"items":[{"memory_id":"a","scope":"project","project_id":"project","path":"a.md","name":"A","description":"","content_hash":"hash","status":"active","updated_at":"now"},{"memory_id":"b","scope":"project","project_id":"project","path":"b.md","name":"B","description":"","content_hash":"hash","status":"active","updated_at":"now"}],"page_info":{"has_more":false,"next_cursor":null}}"#
                }
                return .init(status: 200, headers: [:], body: body)
            }
            let loader = WorkspaceLoader(daemon: daemon, bootstrap: DaemonBootstrapController(), server: server,
                retainBase: { try await probe.retain($0) })
            do {
                let snapshot = try await loader.loadProjectWithMetadata(id: "project", name: "Project")
                XCTAssertFalse(rejects, "An unprotected snapshot must not become available to a Source editor.")
                let pin = try XCTUnwrap(snapshot.state.baseCommitPin)
                XCTAssertEqual(snapshot.resources.count, 2)
                XCTAssertTrue(snapshot.resources.allSatisfy { !$0.contentLoaded && $0.baseCommitPin === pin })
            } catch PinTestError.rejected { XCTAssertTrue(rejects) }
            let acquisitions = await probe.acquisitions
            XCTAssertEqual(acquisitions, 1)
        }
    }

    @MainActor
    func testOnlyActiveDraftsPinHistoricalBaseAndUnavailableDraftKeepsItsTextReadOnly() async throws {
        let daemon = DaemonXPCClient(serviceName: "unused")
        for status in ["open", "submitted", "merged", "discarded"] {
            let probe = SnapshotPinProbe(rejects: true)
            let loader = WorkspaceLoader(daemon: daemon, bootstrap: DaemonBootstrapController(),
                server: ServerClient(daemon: daemon), retainBase: { try await probe.retain($0) })
            let json = """
            {"draft":{"draft_id":"draft","project_id":"project","server_draft_id":null,"server_version":1,
            "base_commit_id":"historical","current_commit_id":"new","freshness":"behind",
            "has_upstream_resource_changes":true,"reconciliation":"unknown","reconciliation_candidate_id":null,
            "scope":"project","resource_kind":"memory","target_id":null,"path":"draft.md",
            "status":"\(status)","created_at":"now","updated_at":"now","pending_operation_count":0,"failed_operation_count":0},
            "operations":[{"local_operation_id":"op","resource_kind":"memory","operation":{"create":{"path":"draft.md","content":{"content":"Retained local text"}}},
            "source":"desktop","sync_status":"synced","last_error":null,"created_at":"now","updated_at":"now"}]}
            """
            let detail = try JSONCoding.decoder().decode(DaemonDraftDetail.self, from: Data(json.utf8))
            let draft = try await loader.loadMappedDraft(detail, resources: [])
            XCTAssertEqual(draft.document.body, "Retained local text")
            XCTAssertTrue(draft.documentBaselineAvailable)
            let active = status == "open" || status == "submitted"
            let acquisitions = await probe.acquisitions
            XCTAssertEqual(acquisitions, active ? 1 : 0, "Terminal history must not reconstruct every old Commit.")
            XCTAssertEqual(draft.baseCommitAvailable, !active)
            if active {
                let sharedProbe = SnapshotPinProbe(rejects: false)
                let batchLoader = WorkspaceLoader(daemon: daemon, bootstrap: DaemonBootstrapController(),
                    server: ServerClient(daemon: daemon), retainBase: { try await sharedProbe.retain($0) })
                let batch = try await batchLoader.loadMappedDrafts([detail, detail, detail], resources: [])
                let batchAcquisitions = await sharedProbe.acquisitions
                XCTAssertEqual(batchAcquisitions, 1, "A batch must only retain each historical Base once.")
                let pin = try XCTUnwrap(batch.first?.baseCommitPin)
                XCTAssertTrue(batch.allSatisfy { $0.baseCommitPin === pin })
            }
            if active {
                let workspace = WorkspaceCoordinator()
                workspace.context.phase = .ready
                workspace.context.activeProjectId = "project"
                let item = MemoryListItem(id: "draft", resource: nil, draft: draft, inherited: false,
                    projectContextId: "project")
                XCTAssertFalse(workspace.edits.canEditMemory(item))
            }
        }
    }

    private func lockFile() throws -> String {
        let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appending(path: "pin.lock")
        try Data().write(to: file)
        return file.path
    }
}

private enum PinTestError: Error { case rejected }

private actor SnapshotPinProbe {
    let rejects: Bool
    private(set) var acquisitions = 0
    init(rejects: Bool) { self.rejects = rejects }
    func retain(_ commitId: String) throws -> CommitBasePin {
        acquisitions += 1
        if rejects { throw PinTestError.rejected }
        return CommitBasePin(commitId: commitId) { }
    }
}

private actor PinRequestProbe {
    let path: String
    let rejectsAcknowledgement: Bool
    let expiresFirstAcknowledgement: Bool
    private(set) var issues = 0
    private(set) var acknowledgedWhileLocked = false

    init(path: String, rejectsAcknowledgement: Bool = false, expiresFirstAcknowledgement: Bool = false) {
        self.path = path
        self.rejectsAcknowledgement = rejectsAcknowledgement
        self.expiresFirstAcknowledgement = expiresFirstAcknowledgement
    }

    func request(_ request: DaemonCommitBasePinRequest) throws -> DaemonCommitBasePinResponse {
        if request.pinId == nil { issues += 1 }
        else {
            acknowledgedWhileLocked = try !canLockExclusively(path)
            if rejectsAcknowledgement { throw PinTestError.rejected }
            if expiresFirstAcknowledgement, issues == 1 {
                throw DaemonXPCError.daemon(.init(code: "commit_base_pin_expired", message: "Expired", requestId: nil))
            }
        }
        return .init(pinId: "pin-\(issues)", commitId: request.commitId, lockPath: path)
    }
}

private func canLockExclusively(_ path: String) throws -> Bool {
    let descriptor = open(path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
    guard descriptor >= 0 else { throw CocoaError(.fileReadUnknown) }
    defer { close(descriptor) }
    return flock(descriptor, LOCK_EX | LOCK_NB) == 0
}
