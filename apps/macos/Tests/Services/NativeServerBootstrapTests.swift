import Foundation
import XCTest
@testable import Clumsies

final class NativeServerBootstrapTests: XCTestCase {
    func testNativeFailuresHaveCorrelatedSanitizedDiagnostics() async throws {
        let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let log = ClientLog(directory: directory)
        for status in [503, 200] {
            NativeSetupURLProtocol.handler = { request in
                XCTAssertEqual(request.value(forHTTPHeaderField: "x-clumsies-request-id"), "req_native_\(status)")
                let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: ["x-request-id": "req_server_\(status)"])!
                return (response, Data("SECRET_INVALID_RESPONSE".utf8))
            }
            let configuration = URLSessionConfiguration.ephemeral
            configuration.protocolClasses = [NativeSetupURLProtocol.self]
            let transport = URLSession(configuration: configuration)
            defer { transport.invalidateAndCancel(); NativeSetupURLProtocol.handler = nil }
            let client = NativeServerSetupClient(origin: try ServerOrigin(validating: "https://clumsies.example.com"), transport: transport)
            await ClientDiagnostics.$testLog.withValue(log) {
                await ClientDiagnostics.$requestID.withValue("req_native_\(status)") {
                    do { _ = try await client.status(); XCTFail("Expected failure") }
                    catch {}
                }
            }
        }
        let content = try String(contentsOf: directory.appending(path: "client.log"), encoding: .utf8)
        for value in ["req_native_503", "req_server_503", "req_native_200", "req_server_200", "http_failed", "decode", "request_failed"] {
            XCTAssertTrue(content.contains(value), "Missing \(value)")
        }
        XCTAssertFalse(content.contains("SECRET_INVALID_RESPONSE"))
    }

    func testServerOriginRequiresHTTPSExceptForLoopbackHTTP() throws {
        XCTAssertEqual(
            try ServerOrigin(validating: "https://clumsies.example.com/").url.absoluteString,
            "https://clumsies.example.com"
        )
        XCTAssertEqual(
            try ServerOrigin(validating: "http://127.9.8.7:49152").url.absoluteString,
            "http://127.9.8.7:49152"
        )
        XCTAssertEqual(
            try ServerOrigin(validating: "http://[::1]:49152").url.host,
            "::1"
        )
        XCTAssertNoThrow(try ServerOrigin(validating: "http://localhost:49152"))

        assertOriginError("http://clumsies.example.com", .insecureRemoteHTTP)
        assertOriginError("ftp://clumsies.example.com", .unsupportedScheme)
        assertOriginError("https://owner:secret@clumsies.example.com", .credentialsNotAllowed)
        assertOriginError("https://clumsies.example.com/setup", .originOnly)
        assertOriginError("https://clumsies.example.com?next=evil", .originOnly)
        assertOriginError("https://clumsies.example.com#fragment", .originOnly)
    }

    func testServerOriginStoreRejectsInvalidPersistedOverride() throws {
        let suite = "NativeServerBootstrapTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let fallback = try ServerOrigin(validating: "https://app.clumsies.ai")
        let store = ServerOriginStore(defaults: defaults, fallback: fallback)

        let selfHosted = try ServerOrigin(validating: "https://memory.example.net:8443")
        store.save(selfHosted)
        XCTAssertEqual(store.load(), selfHosted)

        defaults.set("http://memory.example.net:8080", forKey: ServerOriginStore.defaultsKey)
        XCTAssertEqual(store.load(), fallback)
    }

    func testDesktopAuthorizationURLCarriesStateAndPKCE() throws {
        let parameters = NativeAuthorizationParameters(
            redirectUri: "http://127.0.0.1:54321/callback",
            state: "expected-state",
            codeChallenge: "challenge"
        )
        let url = try AuthenticationClient.authorizationURL(
            serverURL: URL(string: "https://clumsies.example.com")!,
            parameters: parameters
        )
        let components = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
        let query = Dictionary(
            uniqueKeysWithValues: (components.queryItems ?? []).compactMap { item in
                item.value.map { (item.name, $0) }
            }
        )

        XCTAssertEqual(components.path, "/oauth2/authorization/oidc")
        XCTAssertEqual(query["client_kind"], "desktop")
        XCTAssertEqual(query["redirect_uri"], parameters.redirectUri)
        XCTAssertEqual(query["state"], parameters.state)
        XCTAssertEqual(query["code_challenge"], parameters.codeChallenge)
        XCTAssertEqual(query["code_challenge_method"], "S256")
    }

    func testSetupWireRequestUsesSharedStateAndPKCEFieldNames() throws {
        let request = NativeSetupOIDCAuthorizationRequest(
            parameters: .init(
                redirectUri: "http://127.0.0.1:54321/callback",
                state: "expected-state",
                codeChallenge: "challenge"
            )
        )
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONCoding.encoder().encode(request))
                as? [String: String]
        )

        XCTAssertEqual(
            object,
            [
                "redirect_uri": "http://127.0.0.1:54321/callback",
                "state": "expected-state",
                "code_challenge": "challenge",
                "code_challenge_method": "S256",
            ]
        )
    }

    func testMemoryOnlySessionBuildsOriginBoundBearerRequests() throws {
        let session = NativeAuthenticatedSession(
            serverURL: URL(string: "https://clumsies.example.com")!,
            currentUser: currentUser(),
            accessToken: "short-lived-access",
            refreshToken: "short-lived-refresh",
            transport: .shared
        )

        let request = try session.authorizedRequest(
            path: "/api/v1/admin/tokens?limit=5",
            method: "GET"
        )
        XCTAssertEqual(request.url?.absoluteString, "https://clumsies.example.com/api/v1/admin/tokens?limit=5")
        XCTAssertEqual(request.value(forHTTPHeaderField: "authorization"), "Bearer short-lived-access")
        XCTAssertThrowsError(
            try session.authorizedRequest(path: "https://attacker.example/api/v1/admin/tokens")
        )
    }

    func testDirectAuthenticationTransportIsEphemeralAndUncached() {
        let transport = AuthenticationClient.makeEphemeralTransport()
        let configuration = transport.configuration

        XCTAssertEqual(configuration.requestCachePolicy, .reloadIgnoringLocalCacheData)
        XCTAssertNil(configuration.urlCache)
        XCTAssertTrue(configuration.httpShouldSetCookies)
        XCTAssertNotNil(configuration.httpCookieStorage)
        XCTAssertFalse(configuration.httpCookieStorage === HTTPCookieStorage.shared)
    }

    func testSetupTransportRetainsCookieAndSendsCSRFAndPKCE() async throws {
        let recorder = NativeSetupRequestRecorder()
        NativeSetupURLProtocol.handler = { request in
            try recorder.response(for: request)
        }
        defer { NativeSetupURLProtocol.handler = nil }

        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NativeSetupURLProtocol.self]
        configuration.httpShouldSetCookies = true
        configuration.httpCookieAcceptPolicy = .always
        let transport = URLSession(configuration: configuration)
        let origin = try ServerOrigin(validating: "https://clumsies.example.com")
        let client = NativeServerSetupClient(origin: origin, transport: transport)

        let status = try await client.status()
        XCTAssertEqual(status.state, .setupRequired)
        let setupSession = try await client.createSession(setupCode: "setup-secret")
        let saved = try await client.replaceConfiguration(
            .init(
                orgName: "Acme",
                defaultProjectName: "Default",
                allowedEmailDomains: ["example.com"]
            ),
            csrfToken: setupSession.csrfToken
        )
        XCTAssertEqual(saved.orgName, "Acme")
        _ = try await client.createOIDCAuthorization(
            parameters: .init(
                redirectUri: "http://127.0.0.1:54321/callback",
                state: "expected-state",
                codeChallenge: "challenge"
            ),
            csrfToken: setupSession.csrfToken
        )

        let requests = recorder.snapshot()
        XCTAssertEqual(
            requests.map { $0.url?.path },
            [
                "/api/v1/setup",
                "/api/v1/setup/sessions",
                "/api/v1/setup/configuration",
                "/api/v1/setup/oidc-authorizations",
            ]
        )
        XCTAssertEqual(
            requests[2].value(forHTTPHeaderField: "x-csrf-token"),
            "csrf-secret"
        )
        XCTAssertTrue(
            requests[2].value(forHTTPHeaderField: "cookie")?.contains("clumsies_setup=session-secret") == true
        )

        let oidcBody = try XCTUnwrap(requests[3].httpBody)
        let oidc = try XCTUnwrap(
            JSONSerialization.jsonObject(with: oidcBody) as? [String: String]
        )
        XCTAssertEqual(oidc["redirect_uri"], "http://127.0.0.1:54321/callback")
        XCTAssertEqual(oidc["state"], "expected-state")
        XCTAssertEqual(oidc["code_challenge"], "challenge")
        XCTAssertEqual(oidc["code_challenge_method"], "S256")
    }

    func testRecoveryClientReadsHealthAndMutatesMembersAndTokensWithBearer() async throws {
        let recorder = NativeRecoveryRequestRecorder()
        NativeSetupURLProtocol.handler = { request in
            try recorder.response(for: request)
        }
        defer { NativeSetupURLProtocol.handler = nil }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NativeSetupURLProtocol.self]
        let transport = URLSession(configuration: configuration)
        let session = NativeAuthenticatedSession(
            serverURL: URL(string: "https://clumsies.example.com")!,
            currentUser: currentUser(),
            accessToken: "recovery-access",
            refreshToken: "recovery-refresh",
            transport: transport
        )
        let client = NativeAdministratorRecoveryClient(session: session)

        let snapshot = try await client.load()
        XCTAssertEqual(snapshot.health.status, .degraded)
        let member = try XCTUnwrap(snapshot.members.first)
        XCTAssertEqual(member.role, .member)
        let updated = try await client.updateMember(member, role: .admin)
        XCTAssertEqual(updated.role, .admin)
        let token = try XCTUnwrap(snapshot.tokens.first)
        try await client.revokeToken(token)

        let requests = recorder.snapshot()
        XCTAssertTrue(
            requests.allSatisfy {
                $0.value(forHTTPHeaderField: "authorization") == "Bearer recovery-access"
            }
        )
        let memberPatch = try XCTUnwrap(
            requests.first { $0.url?.path == "/api/v1/admin/members/usr_member" }
        )
        XCTAssertEqual(memberPatch.httpMethod, "PATCH")
        XCTAssertEqual(memberPatch.value(forHTTPHeaderField: "If-Match"), "7")
        let body = try XCTUnwrap(memberPatch.httpBody)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: body) as? [String: String])
        XCTAssertEqual(object["role"], "admin")
        XCTAssertEqual(
            requests.first { $0.url?.path == "/api/v1/admin/tokens/tok_1" }?.httpMethod,
            "DELETE"
        )
    }

    private func assertOriginError(
        _ input: String,
        _ expected: ServerOriginError,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertThrowsError(try ServerOrigin(validating: input), file: file, line: line) { error in
            XCTAssertEqual(error as? ServerOriginError, expected, file: file, line: line)
        }
    }

    private func currentUser() -> CurrentUserResponse {
        .init(
            user: .init(
                userId: "usr_owner",
                email: "owner@example.com",
                displayName: "Owner",
                avatarUrl: nil,
                role: "owner"
            ),
            org: .init(orgId: "org_acme", name: "Acme"),
            projects: [],
            defaultProjectId: nil,
            capabilities: ["admin"]
        )
    }
}

private final class NativeRecoveryRequestRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var requests: [URLRequest] = []

    func response(for request: URLRequest) throws -> (HTTPURLResponse, Data) {
        let recorded = materializingBody(in: request)
        lock.withLock { requests.append(recorded) }
        let path = try XCTUnwrap(recorded.url?.path)
        let body: String
        switch (recorded.httpMethod, path) {
        case ("GET", "/api/v1/admin/health"):
            body = #"{"status":"degraded","version":"1.0.0","database":{"status":"ok","message":"ready"},"schema":{"status":"ok","message":"current"},"commit_service":{"status":"degraded","message":"offline"},"oidc":{"status":"ok","message":"configured"}}"#
        case ("GET", "/api/v1/admin/members"):
            body = #"{"items":[{"user_id":"usr_member","email":"member@example.com","display_name":"Member","role":"member","status":"active","external_identity_bound":true,"revision":7}],"page_info":{"next_cursor":null,"has_more":false}}"#
        case ("GET", "/api/v1/admin/tokens"):
            body = #"{"items":[{"token_id":"tok_1","user_id":"usr_member","kind":"refresh","revoked":false,"expires_at":null,"created_at":"2026-08-29T12:00:00Z"}],"page_info":{"next_cursor":null,"has_more":false}}"#
        case ("PATCH", "/api/v1/admin/members/usr_member"):
            body = #"{"user_id":"usr_member","email":"member@example.com","display_name":"Member","role":"admin","status":"active","external_identity_bound":true,"revision":8}"#
        case ("DELETE", "/api/v1/admin/tokens/tok_1"):
            body = #"{"deleted":true,"id":"tok_1"}"#
        default:
            XCTFail("Unexpected recovery request: \(recorded.httpMethod ?? "") \(path)")
            body = "{}"
        }
        return (
            try XCTUnwrap(
                HTTPURLResponse(
                    url: request.url!,
                    statusCode: 200,
                    httpVersion: "HTTP/1.1",
                    headerFields: [:]
                )
            ),
            Data(body.utf8)
        )
    }

    func snapshot() -> [URLRequest] {
        lock.withLock { requests }
    }

    private func materializingBody(in request: URLRequest) -> URLRequest {
        guard request.httpBody == nil, let stream = request.httpBodyStream else { return request }
        stream.open()
        defer { stream.close() }
        var body = Data()
        var buffer = [UInt8](repeating: 0, count: 4_096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            guard count > 0 else { break }
            body.append(buffer, count: count)
        }
        var materialized = request
        materialized.httpBodyStream = nil
        materialized.httpBody = body
        return materialized
    }
}

private final class NativeSetupRequestRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var requests: [URLRequest] = []

    func response(for request: URLRequest) throws -> (HTTPURLResponse, Data) {
        let recorded = Self.materializingBody(in: request)
        lock.withLock { requests.append(recorded) }
        let path = try XCTUnwrap(recorded.url?.path)
        let status: Int
        let headers: [String: String]
        let body: String
        switch path {
        case "/api/v1/setup":
            status = 200
            headers = [:]
            body = #"{"state":"setup_required","setup_code_configured":true,"oidc_configured":true,"session":null}"#
        case "/api/v1/setup/sessions":
            status = 201
            headers = ["Set-Cookie": "clumsies_setup=session-secret; Path=/; HttpOnly; SameSite=Strict"]
            body = #"{"expires_at":"2026-08-29T12:00:00Z","csrf_token":"csrf-secret"}"#
        case "/api/v1/setup/configuration":
            status = 200
            headers = [:]
            body = #"{"org_name":"Acme","default_project_name":"Default","allowed_email_domains":["example.com"]}"#
        case "/api/v1/setup/oidc-authorizations":
            status = 201
            headers = [:]
            body = #"{"authorization_url":"https://id.example.com/authorize"}"#
        default:
            XCTFail("Unexpected request: \(path)")
            status = 404
            headers = [:]
            body = "{}"
        }
        return (
            try XCTUnwrap(
                HTTPURLResponse(
                    url: request.url!,
                    statusCode: status,
                    httpVersion: "HTTP/1.1",
                    headerFields: headers
                )
            ),
            Data(body.utf8)
        )
    }

    private static func materializingBody(in request: URLRequest) -> URLRequest {
        guard request.httpBody == nil, let stream = request.httpBodyStream else { return request }
        stream.open()
        defer { stream.close() }
        var body = Data()
        var buffer = [UInt8](repeating: 0, count: 4_096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            guard count > 0 else { break }
            body.append(buffer, count: count)
        }
        var materialized = request
        materialized.httpBodyStream = nil
        materialized.httpBody = body
        return materialized
    }

    func snapshot() -> [URLRequest] {
        lock.withLock { requests }
    }
}

private final class NativeSetupURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler:
        ((URLRequest) throws -> (HTTPURLResponse, Data))?

    override class func canInit(with request: URLRequest) -> Bool { true }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let handler = Self.handler else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        do {
            let (response, data) = try handler(request)
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}
