import Foundation
import XCTest
@testable import Clumsies

@MainActor
final class ClientFailureTests: XCTestCase {
    func testHTTPAndDaemonFailuresNeverExposeWireMessages() throws {
        let cases: [(Int, ClientFailure)] = [
            (400, .invalidInput), (401, .authentication), (403, .forbidden),
            (404, .missing), (409, .conflict), (422, .invalidInput),
            (429, .rateLimited), (500, .service), (503, .service), (504, .connection)
        ]
        for (status, expected) in cases {
            let error = ServerClientError.response(status: status, message: "SECRET_BODY req_private")
            XCTAssertEqual(ClientFailure(error), expected)
            XCTAssertEqual(error.userFacingMessage, expected.message)
            XCTAssertFalse(error.localizedDescription.contains("SECRET"))
            let payload = try JSONCoding.decoder().decode(APIErrorPayload.self, from: Data("""
                {"code":"server_request_failed","message":"SECRET_BODY","request_id":"req_private","details":{"status":\(status)}}
                """.utf8))
            XCTAssertEqual(ClientFailure(DaemonXPCError.daemon(payload)), expected)
            XCTAssertEqual(DaemonXPCError.daemon(payload).localizedDescription, expected.message)
        }
        XCTAssertEqual(DaemonXPCError.requestTimedOut(timeout: 30).userFacingMessage, ClientFailure.timeout.message)
        XCTAssertEqual(ServerClientError.invalidResponse("SECRET_DECODING_PATH").userFacingMessage,
                       ClientFailure.invalidResponse.message)
        XCTAssertEqual(NSError(domain: "private_domain", code: 99,
            userInfo: [NSLocalizedDescriptionKey: "SECRET"]).userFacingMessage, ClientFailure.other.message)
    }

    func testCancellationIsSilentAndLocalRecoveryInstructionsSurvive() {
        for error: Error in [CancellationError(), URLError(.cancelled), CocoaError(.userCancelled)] {
            XCTAssertTrue(error.isUserCancellation)
            XCTAssertNil(error.actionMessage)
            XCTAssertNil(error.backgroundMessage)
        }
        XCTAssertNil(URLError(.notConnectedToInternet).backgroundMessage)
        XCTAssertEqual(URLError(.notConnectedToInternet).actionMessage, ClientFailure.connection.message)
        XCTAssertEqual(CocoaError(.fileWriteOutOfSpace).userFacingMessage, ClientFailure.storageFull.message)
        XCTAssertEqual(CocoaError(.fileReadNoPermission).userFacingMessage, ClientFailure.fileAccess.message)
        XCTAssertEqual(DocumentSyncError.mutationWhileSynchronizing.userFacingMessage,
                       DocumentSyncError.mutationWhileSynchronizing.errorDescription)
    }

    func testConnectionStatusRejectsLateCompletionsAndRecoversWithoutCrossingAuthority() {
        let status = ClientServiceStatus()
        let old = status.begin("server:GET:/one")
        let current = status.begin("server:GET:/one")
        status.finish("server:GET:/one", token: current, failure: nil)
        status.finish("server:GET:/one", token: old, failure: .connection)
        XCTAssertNil(status.failure)
        status.finish("server:GET:/one", token: status.begin("server:GET:/one"), failure: .connection)
        status.finish("daemon:health", token: status.begin("daemon:health"), failure: nil)
        XCTAssertEqual(status.failure, .connection, "A healthy local service says nothing about remote connectivity.")
        status.finish("server:GET:/two", token: status.begin("server:GET:/two"), failure: .cancelled)
        XCTAssertEqual(status.failure, .connection)
        status.finish("server:GET:/two", token: status.begin("server:GET:/two"), failure: nil)
        XCTAssertNil(status.failure)
        status.finish("server:GET:/one", token: status.begin("server:GET:/one"), failure: .service)
        status.finish("server:GET:/two", token: status.begin("server:GET:/two"), failure: nil)
        XCTAssertEqual(status.failure, .service, "Do not report a failed endpoint as healthy based on another endpoint.")
        status.dismiss()
        XCTAssertNil(status.failure)
        let previousAccount = status.begin("server:GET:/one")
        status.reset()
        status.finish("server:GET:/one", token: previousAccount, failure: .authentication)
        XCTAssertNil(status.failure)
    }

    func testTransportReportsServiceFailuresButNotMissingResources() async throws {
        ClientServiceStatus.shared.reset()
        defer { ClientServiceStatus.shared.reset() }
        let daemon = DaemonXPCClient(serviceName: "unused")
        let offline = ServerClient(daemon: daemon, sendRequest: { _ in throw URLError(.notConnectedToInternet) })
        do {
            let _: EmptyPayload = try await offline.get("/api/v1/me/inbox")
            XCTFail("Expected connection failure")
        } catch { XCTAssertEqual(ClientFailure(error), .connection) }
        XCTAssertEqual(ClientServiceStatus.shared.failure, .connection)
        let online = ServerClient(daemon: daemon, sendRequest: { _ in
            .init(status: 200, headers: [:], body: "{}")
        })
        let _: EmptyPayload = try await online.get("/api/v1/me/inbox")
        XCTAssertNil(ClientServiceStatus.shared.failure)
        let missing = ServerClient(daemon: daemon, sendRequest: { _ in
            .init(status: 404, headers: [:], body: "PRIVATE_BODY")
        })
        do {
            let _: EmptyPayload = try await missing.get("/api/v1/reviews/deleted")
            XCTFail("Expected missing resource")
        } catch { XCTAssertEqual(error.userFacingMessage, ClientFailure.missing.message) }
        XCTAssertNil(ClientServiceStatus.shared.failure)
    }

    func testRefreshFailureKeepsWorkspaceButColdStartShowsRecovery() {
        let coordinator = WorkspaceCoordinator()
        let context = coordinator.context
        context.account = .init(userId: "test", email: "test@example.com", displayName: nil, avatarUrl: nil, role: "member")
        let generation = context.workspaceReloadGeneration, authority = context.authorityGeneration
        context.phase = .loading
        coordinator.finishFailedReload(URLError(.timedOut), generation: generation,
            authority: authority, previousPhase: .ready)
        XCTAssertEqual(context.phase, .ready)
        XCTAssertEqual(context.account?.userId, "test")
        XCTAssertNil(coordinator.feedback.errorMessage)
        context.account = nil
        coordinator.finishFailedReload(URLError(.timedOut), generation: generation,
            authority: authority, previousPhase: .launching)
        XCTAssertEqual(context.phase, .failed(ClientFailure.connection.message))
        context.authorityGeneration = UUID()
        context.phase = .authenticationRequired
        coordinator.finishFailedReload(URLError(.timedOut), generation: generation,
            authority: authority, previousPhase: .ready)
        XCTAssertEqual(context.phase, .authenticationRequired)
    }

    func testFailedSaveReachesTheFormWithoutAlsoBecomingWindowConnectionFeedback() async throws {
        ClientServiceStatus.shared.reset()
        defer { ClientServiceStatus.shared.reset() }
        let server = ServerClient(daemon: DaemonXPCClient(serviceName: "unused"), sendRequest: { _ in
            .init(status: 500, headers: [:], body: "PRIVATE_BODY")
        })
        do {
            let _: EmptyPayload = try await server.send(method: "PATCH", path: "/api/v1/admin/org", body: EmptyPayload())
            XCTFail("A failed Save must not succeed.")
        } catch { XCTAssertEqual(error.actionMessage, ClientFailure.service.message) }
        XCTAssertNil(ClientServiceStatus.shared.failure, "The form owns the failed submission; do not repeat it behind the sheet.")
        do {
            let _: EmptyPayload = try await server.get("/api/v1/admin/org")
            XCTFail("Expected refresh failure.")
        } catch { XCTAssertEqual(error.userFacingMessage, ClientFailure.service.message) }
        XCTAssertEqual(ClientServiceStatus.shared.failure, .service, "Read failures must still maintain connection status.")
    }

    func testFailedReviewSubmissionRetainsInputAndDoesNotRetryWrites() async {
        var attempts = 0
        let model = ReviewRequestModel(initialTitle: "My review", loadCandidates: { [] }, onSubmit: { _, _, _, _ in
            attempts += 1
            throw ServerClientError.response(status: 500, message: "SECRET")
        })
        model.description = "Keep this description."
        let succeeded = await model.submit()
        XCTAssertFalse(succeeded)
        XCTAssertEqual(attempts, 1)
        XCTAssertEqual(model.title, "My review")
        XCTAssertEqual(model.description, "Keep this description.")
        XCTAssertEqual(model.errorMessage, ClientFailure.service.message)
        XCTAssertFalse(model.isSubmitting)
    }
}
