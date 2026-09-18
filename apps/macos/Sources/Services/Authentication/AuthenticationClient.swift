import AppKit
import CryptoKit
import Darwin
import Foundation

enum AuthenticationError: LocalizedError, Sendable {
    case callbackServer(String)
    case invalidAuthorizationURL
    case invalidRequestPath
    case browserLaunchFailed
    case callbackTimedOut
    case invalidCallback
    case stateMismatch
    case provider(String)
    case server(status: Int, message: String)

    var errorDescription: String? {
        switch self {
        case .callbackServer(let message): message
        case .invalidAuthorizationURL: "Could not create the organization sign-in URL."
        case .invalidRequestPath: "The authenticated Server request path is invalid."
        case .browserLaunchFailed: "Could not open the system browser."
        case .callbackTimedOut: "Organization sign-in timed out."
        case .invalidCallback: "The organization sign-in callback is invalid."
        case .stateMismatch: "The organization sign-in state did not match."
        case .provider(let message): "Organization sign-in failed: \(message)"
        case .server(let status, let message): "Server authentication failed (\(status)): \(message)"
        }
    }
}

struct NativeAuthorizationParameters: Equatable, Encodable, Sendable {
    let redirectUri: String
    let state: String
    let codeChallenge: String
    let codeChallengeMethod = "S256"
}

struct NativeAuthorizationGrant: Sendable {
    let code: String
    let redirectURI: String
    let verifier: String
}

struct NativeBrowserAuthorizationFlow: @unchecked Sendable {
    let parameters: NativeAuthorizationParameters

    private let verifier: String
    private let callback: LoopbackCallbackServer

    static func start() throws -> Self {
        let callback = try LoopbackCallbackServer.open()
        let verifier = randomSecret()
        return .init(
            parameters: .init(
                redirectUri: "http://127.0.0.1:\(callback.port)/callback",
                state: randomSecret(),
                codeChallenge: base64URL(Data(SHA256.hash(data: Data(verifier.utf8))))
            ),
            verifier: verifier,
            callback: callback
        )
    }

    func authorize(at authorizationURL: URL) async throws -> NativeAuthorizationGrant {
        let didOpen = await MainActor.run { NSWorkspace.shared.open(authorizationURL) }
        guard didOpen else {
            callback.close()
            throw AuthenticationError.browserLaunchFailed
        }
        let code = try await callback.waitForCode(expectedState: parameters.state)
        return .init(code: code, redirectURI: parameters.redirectUri, verifier: verifier)
    }

    func cancel() {
        callback.close()
    }

    private static func randomSecret() -> String {
        UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
            + UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
    }

    private static func base64URL(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}

/// A short-lived, App-memory-only Server session for setup and daemon-down recovery.
/// Tokens are never exposed as properties or persisted by this type.
struct NativeAuthenticatedSession: @unchecked Sendable {
    let serverURL: URL
    let currentUser: CurrentUserResponse

    private let accessToken: String
    private let refreshToken: String
    private let transport: URLSession

    init(
        serverURL: URL,
        currentUser: CurrentUserResponse,
        accessToken: String,
        refreshToken: String,
        transport: URLSession
    ) {
        self.serverURL = serverURL
        self.currentUser = currentUser
        self.accessToken = accessToken
        self.refreshToken = refreshToken
        self.transport = transport
    }

    func install(on daemon: DaemonXPCClient) async throws -> DaemonProjectConfig {
        try await daemon.replaceProjectConfig(
            .init(
                serverUrl: serverURL.absoluteString,
                projectId: currentUser.defaultProjectId ?? currentUser.projects.first?.projectId,
                accessToken: accessToken,
                refreshToken: refreshToken
            )
        )
    }

    func authorizedRequest(
        path: String,
        method: String = "GET",
        headers: [String: String] = [:],
        body: Data? = nil
    ) throws -> URLRequest {
        guard path.hasPrefix("/api/v1/"),
              let route = URLComponents(string: path),
              route.scheme == nil,
              route.host == nil,
              var components = URLComponents(url: serverURL, resolvingAgainstBaseURL: false) else {
            throw AuthenticationError.invalidRequestPath
        }
        components.percentEncodedPath = route.percentEncodedPath
        components.percentEncodedQuery = route.percentEncodedQuery
        guard let url = components.url else { throw AuthenticationError.invalidRequestPath }

        var request = URLRequest(url: url)
        request.httpMethod = method
        request.httpBody = body
        request.setValue("Bearer \(accessToken)", forHTTPHeaderField: "authorization")
        for (name, value) in headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        if body != nil {
            request.setValue("application/json", forHTTPHeaderField: "content-type")
        }
        return request
    }

    func data(
        path: String,
        method: String = "GET",
        headers: [String: String] = [:],
        body: Data? = nil
    ) async throws -> Data {
        let request = try authorizedRequest(
            path: path,
            method: method,
            headers: headers,
            body: body
        )
        let (data, response) = try await ClientDiagnostics.data(for: request, using: transport)
        try AuthenticationClient.validate(response: response, data: data)
        return data
    }

    func decode<Response: Decodable & Sendable>(
        _ type: Response.Type,
        path: String,
        method: String = "GET",
        headers: [String: String] = [:],
        body: Data? = nil
    ) async throws -> Response {
        try await ClientDiagnostics.operation(layer: "native_http", method: method) {
            let data = try await data(path: path, method: method, headers: headers, body: body)
            return try JSONCoding.decoder().decode(type, from: data)
        }
    }
}

struct AuthenticationClient: @unchecked Sendable {
    private let origin: URL
    private let transport: URLSession

    init(
        serverURL: URL = ClumsiesIdentifiers.serverURL,
        transport: URLSession? = nil
    ) {
        origin = serverURL
        self.transport = transport ?? Self.makeEphemeralTransport()
    }

    func authenticate() async throws -> NativeAuthenticatedSession {
        let flow = try NativeBrowserAuthorizationFlow.start()
        defer { flow.cancel() }
        let authorizationURL = try Self.authorizationURL(
            serverURL: origin,
            parameters: flow.parameters
        )
        return try await authenticate(using: flow.authorize(at: authorizationURL))
    }

    func authenticate(using grant: NativeAuthorizationGrant) async throws
        -> NativeAuthenticatedSession {
        let tokens = try await exchangeCode(grant)
        let currentUser = try await loadCurrentUser(accessToken: tokens.accessToken)
        return .init(
            serverURL: origin,
            currentUser: currentUser,
            accessToken: tokens.accessToken,
            refreshToken: tokens.refreshToken,
            transport: transport
        )
    }

    static func authorizationURL(
        serverURL: URL,
        parameters: NativeAuthorizationParameters
    ) throws -> URL {
        var components = URLComponents(
            url: serverURL.appending(path: "/oauth2/authorization/oidc"),
            resolvingAgainstBaseURL: false
        )
        components?.queryItems = [
            .init(name: "client_kind", value: "desktop"),
            .init(name: "redirect_uri", value: parameters.redirectUri),
            .init(name: "code_challenge", value: parameters.codeChallenge),
            .init(name: "code_challenge_method", value: parameters.codeChallengeMethod),
            .init(name: "state", value: parameters.state),
        ]
        guard let url = components?.url else {
            throw AuthenticationError.invalidAuthorizationURL
        }
        return url
    }

    private func exchangeCode(_ grant: NativeAuthorizationGrant) async throws -> TokenResponse {
        var request = URLRequest(url: origin.appending(path: "/api/v1/auth/token"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        request.httpBody = try JSONCoding.encoder().encode(
            TokenExchangeRequest(
                code: grant.code,
                redirectUri: grant.redirectURI,
                codeVerifier: grant.verifier
            )
        )
        return try await send(request)
    }

    private func loadCurrentUser(accessToken: String) async throws -> CurrentUserResponse {
        var request = URLRequest(url: origin.appending(path: "/api/v1/me"))
        request.setValue("Bearer \(accessToken)", forHTTPHeaderField: "authorization")
        return try await send(request)
    }

    private func send<Response: Decodable & Sendable>(_ request: URLRequest) async throws -> Response {
        try await ClientDiagnostics.operation(layer: "native_http", method: request.httpMethod ?? "GET") {
            let (data, response) = try await ClientDiagnostics.data(for: request, using: transport)
            try Self.validate(response: response, data: data)
            return try JSONCoding.decoder().decode(Response.self, from: data)
        }
    }

    static func validate(response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else {
            throw AuthenticationError.server(status: 0, message: "No HTTP response was returned.")
        }
        guard (200..<300).contains(http.statusCode) else {
            let apiError = try? JSONCoding.decoder().decode(APIErrorPayload.self, from: data)
            let message = apiError.map { "\($0.code): \($0.message)" }
                ?? String(decoding: data, as: UTF8.self)
            throw AuthenticationError.server(status: http.statusCode, message: message)
        }
    }

    static func makeEphemeralTransport() -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.urlCache = nil
        configuration.httpShouldSetCookies = true
        configuration.httpCookieAcceptPolicy = .always
        return URLSession(configuration: configuration)
    }
}

private final class LoopbackCallbackServer: @unchecked Sendable {
    let descriptor: Int32
    let port: UInt16

    private let closeLock = NSLock()
    private var isClosed = false

    private init(descriptor: Int32, port: UInt16) {
        self.descriptor = descriptor
        self.port = port
    }

    static func open() throws -> LoopbackCallbackServer {
        let descriptor = socket(AF_INET, SOCK_STREAM, 0)
        guard descriptor >= 0 else {
            throw AuthenticationError.callbackServer("Could not create the local callback socket.")
        }
        var reuse: Int32 = 1
        setsockopt(descriptor, SOL_SOCKET, SO_REUSEADDR, &reuse, socklen_t(MemoryLayout<Int32>.size))

        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = 0
        address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
        let bindResult = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { addressPointer in
                Darwin.bind(descriptor, addressPointer, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bindResult == 0, listen(descriptor, 1) == 0 else {
            Darwin.close(descriptor)
            throw AuthenticationError.callbackServer("Could not bind the local callback socket.")
        }

        var boundAddress = sockaddr_in()
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let nameResult = withUnsafeMutablePointer(to: &boundAddress) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { addressPointer in
                getsockname(descriptor, addressPointer, &length)
            }
        }
        guard nameResult == 0 else {
            Darwin.close(descriptor)
            throw AuthenticationError.callbackServer("Could not read the local callback port.")
        }
        return .init(descriptor: descriptor, port: UInt16(bigEndian: boundAddress.sin_port))
    }

    func close() {
        closeLock.withLock {
            guard !isClosed else { return }
            isClosed = true
            Darwin.close(descriptor)
        }
    }

    func waitForCode(expectedState: String) async throws -> String {
        try await Task.detached(priority: .userInitiated) { [self] in
            defer { close() }
            var pollDescriptor = pollfd(fd: descriptor, events: Int16(POLLIN), revents: 0)
            guard poll(&pollDescriptor, 1, 300_000) > 0 else {
                throw AuthenticationError.callbackTimedOut
            }
            let client = accept(descriptor, nil, nil)
            guard client >= 0 else {
                throw AuthenticationError.invalidCallback
            }
            defer { Darwin.close(client) }

            var buffer = [UInt8](repeating: 0, count: 16_384)
            let count = recv(client, &buffer, buffer.count, 0)
            guard count > 0,
                  let request = String(bytes: buffer.prefix(count), encoding: .utf8),
                  let requestLine = request.split(separator: "\r\n", maxSplits: 1).first else {
                Self.respond(to: client, success: false)
                throw AuthenticationError.invalidCallback
            }
            let components = requestLine.split(separator: " ")
            guard components.count >= 2,
                  components[0] == "GET",
                  let callback = URLComponents(string: "http://127.0.0.1\(components[1])"),
                  callback.path == "/callback" else {
                Self.respond(to: client, success: false)
                throw AuthenticationError.invalidCallback
            }
            let values = Dictionary(
                callback.queryItems?.compactMap { item in item.value.map { (item.name, $0) } } ?? [],
                uniquingKeysWith: { first, _ in first }
            )
            guard values["state"] == expectedState else {
                Self.respond(to: client, success: false)
                throw AuthenticationError.stateMismatch
            }
            if let providerError = values["error"] {
                Self.respond(to: client, success: false)
                throw AuthenticationError.provider(values["error_description"] ?? providerError)
            }
            guard let code = values["code"] else {
                Self.respond(to: client, success: false)
                throw AuthenticationError.invalidCallback
            }
            Self.respond(to: client, success: true)
            return code
        }.value
    }

    private static func respond(to descriptor: Int32, success: Bool) {
        let status = success ? "200 OK" : "400 Bad Request"
        let title = success ? "Signed in to Clumsies" : "Sign-in failed"
        let message = success
            ? "You can close this window and return to the Clumsies app."
            : "Return to the Clumsies app to review the error."
        let body = "<!doctype html><meta charset=\"utf-8\"><title>\(title)</title><h1>\(title)</h1><p>\(message)</p>"
        let response = "HTTP/1.1 \(status)\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: \(body.utf8.count)\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n\(body)"
        _ = response.withCString { pointer in
            Darwin.send(descriptor, pointer, strlen(pointer), 0)
        }
    }
}
