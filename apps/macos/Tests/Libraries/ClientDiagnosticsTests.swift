import Foundation
import XCTest
@testable import Clumsies

final class ClientDiagnosticsTests: XCTestCase {
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
                    let _: Void = try await ClientDiagnostics.operation(layer: "xpc", method: "server_request") {
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
