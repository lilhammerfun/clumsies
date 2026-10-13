import Foundation
import XCTest
@testable import Clumsies

final class ClientDiagnosticsTests: XCTestCase {
    func testSuccessfulBindingRequestLeavesStartAndCompletionEvidence() async throws {
        let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        try await ClientDiagnostics.$testLog.withValue(ClientLog(directory: directory)) {
            try await ClientDiagnostics.$requestID.withValue("req_binding") {
                let result = try await ClientDiagnostics.operation(layer: "xpc", method: "remove_project_binding", recordLifecycle: true) { true }
                XCTAssertTrue(result)
            }
        }
        let content = try String(contentsOf: directory.appending(path: "client.log"), encoding: .utf8)
        for event in ["request_started", "request_completed", "req_binding", "remove_project_binding"] {
            XCTAssertTrue(content.contains(event))
        }
        XCTAssertEqual(ClientDiagnostics.workspaceID("/private/workspace/../workspace"), ClientDiagnostics.workspaceID("/private/workspace"))
    }

    func testFailureEvidencePreservesRequestAndHidesErrorContents() async throws {
        let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let log = ClientLog(directory: directory)
        let payload = try JSONCoding.decoder().decode(APIErrorPayload.self, from: Data("""
        {"code":"server_request_failed","message":"SECRET_BODY","request_id":"req_demo","details":{"timeout":true,"causes":[{"kind":"timeout"}]}}
        """.utf8))
        await ClientDiagnostics.$testLog.withValue(log) {
            await ClientDiagnostics.$requestID.withValue("req_demo") {
                do {
                    let _: Void = try await ClientDiagnostics.operation(layer: "xpc", method: "server_request", recordLifecycle: true) {
                        throw DaemonXPCError.daemon(payload)
                    }
                    XCTFail("Expected failure")
                } catch {}
            }
        }
        let extended = try JSONCoding.decoder().decode(APIErrorPayload.self, from: Data("""
        {"code":"validation_failed","message":"preserved","request_id":"req_extended","details":{"body":{"field":"required"}}}
        """.utf8))
        XCTAssertEqual(extended.message, "preserved")
        XCTAssertEqual(extended.requestId, "req_extended")
        let content = try String(contentsOf: directory.appending(path: "client.log"), encoding: .utf8)
        XCTAssertTrue(content.contains("request_failed"))
        XCTAssertTrue(content.contains("request_started"))
        XCTAssertFalse(content.contains("request_completed"))
        XCTAssertTrue(content.contains("req_demo"))
        XCTAssertTrue(content.contains("timeout"))
        XCTAssertFalse(content.contains("SECRET_BODY"))
        XCTAssertEqual(ClientDiagnostics.route("/api/v1/projects/private/path?token=secret"), "/api/v1/projects")
        XCTAssertEqual(ClientDiagnostics.route("/api/v1/private"), "/api/v1/:resource")
    }

    func testServerDecodeFailureRetainsBothIDsWithoutResponseBody() throws {
        let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let log = ClientLog(directory: directory)
        let client = ServerClient(daemon: DaemonXPCClient(serviceName: "unused"))
        let response = DaemonServerResponse(status: 200, headers: ["x-request-id": "req_server_decode", "x-clumsies-request-id": "req_client_decode"], body: "SECRET_RESPONSE")
        ClientDiagnostics.$testLog.withValue(log) {
            do {
                let _: EmptyPayload = try client.decode(response, method: "POST", path: "/api/v1/reviews?private=SECRET_QUERY")
                XCTFail("Expected decode failure")
            } catch {}
        }
        let content = try String(contentsOf: directory.appending(path: "client.log"), encoding: .utf8)
        for value in ["response_decode_failed", "req_server_decode", "req_client_decode"] { XCTAssertTrue(content.contains(value)) }
        XCTAssertFalse(content.contains("SECRET"))
    }

    func testMissingXPCServiceLeavesFailureEvidence() async throws {
        let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let log = ClientLog(directory: directory)
        await ClientDiagnostics.$testLog.withValue(log) {
            await ClientDiagnostics.$requestID.withValue("req_missing_xpc") {
                do {
                    _ = try await DaemonXPCClient(serviceName: "ai.clumsies.test.missing.\(UUID().uuidString)").health(timeout: 0.1)
                    XCTFail("Expected missing service failure")
                } catch {}
            }
        }
        let content = try String(contentsOf: directory.appending(path: "client.log"), encoding: .utf8)
        XCTAssertTrue(content.contains("req_missing_xpc"))
        XCTAssertTrue(content.contains("request_failed"))
        XCTAssertTrue(content.contains("xpc_"))
    }

    func testPerformanceContractAndExportPreserveCountersAndLegacyAbsence() throws {
        let legacy = Data("""
        {"daemon_version":"0.20.7","agent_runtime":{"protocol_revision":1,"build_id":"test"},"server_url":"http://127.0.0.1","project_id":null,"daemon_installation_id":"test","log_dir":"/private/tmp/test","local_db":{"path":"local.db","ready":true,"schema_version":1}}
        """.utf8)
        XCTAssertNil(try JSONCoding.decoder().decode(DaemonHealth.self, from: legacy).performance)
        let snapshot = try JSONCoding.decoder().decode(DaemonPerformanceSnapshot.self, from: Data("""
        {"process_id":42,"collection_started_at":"2026-10-13T00:00:00Z","snapshot_at":"2026-10-13T00:00:01Z","collection_elapsed_ms":1000,"bucket_upper_bounds_ms":[5,10],"operations":{"model_gate_wait":{"started":2,"in_flight":0,"outcomes":{"success":{"count":1,"total_us":8000,"buckets":[0,1]},"interrupted":{"count":1,"total_us":2000,"buckets":[1,1]}}}}}
        """.utf8))
        XCTAssertEqual(snapshot.operations["model_gate_wait"]?.outcomes["success"]?.buckets, [0, 1])
        let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let destination = directory.appending(path: "export")
        try ClientDiagnostics.export(to: destination, appDirectory: directory, daemonDirectory: directory, metadata: [:], performance: snapshot)
        let exported = try Data(contentsOf: destination.appending(path: "performance.json"))
        XCTAssertEqual(try JSONCoding.decoder().decode(DaemonPerformanceSnapshot.self, from: exported), snapshot)
        let manifest = try JSONSerialization.jsonObject(with: Data(contentsOf: destination.appending(path: "manifest.json"))) as? [String: String]
        XCTAssertEqual(manifest?["performance.json"], "included; process-scoped counters")
        let fields = try XCTUnwrap(JSONSerialization.jsonObject(with: exported) as? [String: Any])
        for field in ["query", "project_id", "request_id", "content", "token"] { XCTAssertNil(fields[field]) }
    }

    func testSyncTimingFieldsDecodeWhenPresentAndRemainOptionalForLegacy() throws {
        let legacy = Data("""
        {"unavailable_projects":[],"draft_sync":{"state":"idle"},"commit_sync":{"state":"idle"},"pending_operation_count":0,"failed_operation_count":0,"behind_draft_count":0,"reconciliation_conflict_count":0}
        """.utf8)
        let old = try JSONCoding.decoder().decode(DaemonSyncStatus.self, from: legacy)
        XCTAssertNil(old.oldestPendingOperationAt)
        XCTAssertNil(old.commitSync.lastCheckedSuccessAt)
        let channel = try JSONCoding.decoder().decode(DaemonSyncChannelStatus.self, from: Data("""
        {"state":"idle","last_checked_success_at":"2026-10-13T00:00:00Z","last_success_at":"2026-10-12T00:00:00Z"}
        """.utf8))
        XCTAssertNotEqual(channel.lastCheckedSuccessAt, channel.lastSuccessAt)
    }

    func testRotationAndExportIncludeOnlyBoundedLogsAndManifest() throws {
        let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let logs = directory.appending(path: "logs")
        try FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true)
        try Data(repeating: 65, count: 1024).write(to: logs.appending(path: "client.log"))
        try Data(repeating: 65, count: 1024).write(to: logs.appending(path: "client.log.3"))
        let log = ClientLog(directory: logs, limit: 256)
        for index in 0..<40 { log.record("failure", ["request_id": "req_\(index)"]) }
        let files = try FileManager.default.contentsOfDirectory(at: logs, includingPropertiesForKeys: [.fileSizeKey])
        XCTAssertEqual(files.count, 4)
        for file in files { XCTAssertLessThanOrEqual(try Data(contentsOf: file).count, 256) }
        try Data("DO_NOT_EXPORT_DATABASE".utf8).write(to: logs.appending(path: "state.db"))
        let destination = directory.appending(path: "export")
        try ClientDiagnostics.export(to: destination, appDirectory: logs, daemonDirectory: logs, metadata: ["instance": "test"])
        let exported = try FileManager.default.contentsOfDirectory(atPath: destination.path)
        XCTAssertEqual(exported.count, 5)
        XCTAssertFalse(exported.contains("state.db"))
        let manifest = try String(contentsOf: destination.appending(path: "manifest.json"), encoding: .utf8)
        XCTAssertTrue(manifest.contains("missing"))
        XCTAssertTrue(manifest.contains("test"))
        XCTAssertThrowsError(try ClientDiagnostics.export(to: destination, appDirectory: logs, daemonDirectory: logs, metadata: [:]))
    }
}
