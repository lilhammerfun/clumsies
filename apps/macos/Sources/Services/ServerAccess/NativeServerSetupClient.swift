import Foundation

enum NativeInstallationState: String, Codable, Sendable {
    case setupRequired = "setup_required"
    case initialized
}

struct NativeSetupConfiguration: Codable, Equatable, Sendable {
    let orgName: String
    let defaultProjectName: String
    let allowedEmailDomains: [String]
}

struct NativeSetupSessionStatus: Codable, Equatable, Sendable {
    let expiresAt: String
    let configuration: NativeSetupConfiguration?
}

struct NativeSetupStatus: Codable, Equatable, Sendable {
    let state: NativeInstallationState
    let setupCodeConfigured: Bool
    let oidcConfigured: Bool
    let session: NativeSetupSessionStatus?
}

struct NativeSetupSessionRequest: Encodable, Sendable {
    let setupCode: String
}

struct NativeSetupSessionResponse: Decodable, Sendable {
    let expiresAt: String
    let csrfToken: String
}

struct NativeSetupOIDCAuthorizationRequest: Encodable, Equatable, Sendable {
    let redirectUri: String
    let state: String
    let codeChallenge: String
    let codeChallengeMethod: String

    init(parameters: NativeAuthorizationParameters) {
        redirectUri = parameters.redirectUri
        state = parameters.state
        codeChallenge = parameters.codeChallenge
        codeChallengeMethod = parameters.codeChallengeMethod
    }
}

struct NativeSetupOIDCAuthorizationResponse: Decodable, Sendable {
    let authorizationUrl: String
}

enum NativeServerSetupError: UserFacingError, Sendable {
    case noSetupCode
    case oidcNotConfigured
    case alreadyInitialized
    case invalidAuthorizationURL
    case server(status: Int, message: String)

    var errorDescription: String? {
        switch self {
        case .noSetupCode:
            String(localized: "This Server requires setup, but its deployment has no CLUMSIES_SETUP_CODE configured.")
        case .oidcNotConfigured:
            String(localized: "Configure the Server's OIDC deployment settings before completing setup.")
        case .alreadyInitialized:
            String(localized: "This Server has already been set up. Sign in instead.")
        case .invalidAuthorizationURL:
            String(localized: "The Server returned an invalid identity-provider URL.")
        case .server(let status, _):
            ClientFailure(status: status).message
        }
    }
}

struct NativeServerSetupClient: @unchecked Sendable {
    let origin: ServerOrigin

    private let transport: URLSession

    init(origin: ServerOrigin, transport: URLSession? = nil) {
        self.origin = origin
        self.transport = transport ?? Self.makeEphemeralTransport()
    }

    func status() async throws -> NativeSetupStatus {
        try await send(URLRequest(url: endpoint("/api/v1/setup")))
    }

    func completeSetup(
        setupCode: String,
        configuration: NativeSetupConfiguration
    ) async throws -> NativeAuthenticatedSession {
        let status = try await status()
        guard status.state == .setupRequired else {
            throw NativeServerSetupError.alreadyInitialized
        }
        guard status.setupCodeConfigured else {
            throw NativeServerSetupError.noSetupCode
        }
        guard status.oidcConfigured else {
            throw NativeServerSetupError.oidcNotConfigured
        }

        let setupSession = try await createSession(setupCode: setupCode)
        _ = try await replaceConfiguration(
            configuration,
            csrfToken: setupSession.csrfToken
        )

        let flow = try NativeBrowserAuthorizationFlow.start()
        defer { flow.cancel() }
        let authorization = try await createOIDCAuthorization(
            parameters: flow.parameters,
            csrfToken: setupSession.csrfToken
        )
        guard let authorizationURL = URL(string: authorization.authorizationUrl),
              let scheme = authorizationURL.scheme?.lowercased(),
              scheme == "https"
                || (scheme == "http" && authorizationURL.host.map(ServerOrigin.isLoopback) == true)
        else {
            throw NativeServerSetupError.invalidAuthorizationURL
        }
        let grant = try await flow.authorize(at: authorizationURL)
        return try await AuthenticationClient(
            serverURL: origin.url,
            transport: transport
        ).authenticate(using: grant)
    }

    func createSession(setupCode: String) async throws -> NativeSetupSessionResponse {
        try await sendJSON(
            path: "/api/v1/setup/sessions",
            method: "POST",
            body: NativeSetupSessionRequest(setupCode: setupCode)
        )
    }

    func replaceConfiguration(
        _ configuration: NativeSetupConfiguration,
        csrfToken: String
    ) async throws -> NativeSetupConfiguration {
        try await sendJSON(
            path: "/api/v1/setup/configuration",
            method: "PUT",
            body: configuration,
            csrfToken: csrfToken
        )
    }

    func createOIDCAuthorization(
        parameters: NativeAuthorizationParameters,
        csrfToken: String
    ) async throws -> NativeSetupOIDCAuthorizationResponse {
        try await sendJSON(
            path: "/api/v1/setup/oidc-authorizations",
            method: "POST",
            body: NativeSetupOIDCAuthorizationRequest(parameters: parameters),
            csrfToken: csrfToken
        )
    }

    private func endpoint(_ path: String) -> URL {
        origin.url.appending(path: path)
    }

    private func sendJSON<Body: Encodable & Sendable, Response: Decodable & Sendable>(
        path: String,
        method: String,
        body: Body,
        csrfToken: String? = nil
    ) async throws -> Response {
        var request = URLRequest(url: endpoint(path))
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        if let csrfToken {
            request.setValue(csrfToken, forHTTPHeaderField: "x-csrf-token")
        }
        request.httpBody = try JSONCoding.encoder().encode(body)
        return try await send(request)
    }

    private func send<Response: Decodable & Sendable>(_ originalRequest: URLRequest) async throws
        -> Response {
        try await ClientDiagnostics.operation(layer: "native_http", method: originalRequest.httpMethod ?? "GET") {
            var request = originalRequest
            let cookieStorage = transport.configuration.httpCookieStorage
            if request.value(forHTTPHeaderField: "cookie") == nil,
               let url = request.url,
               let cookies = cookieStorage?.cookies(for: url),
               !cookies.isEmpty {
                for (name, value) in HTTPCookie.requestHeaderFields(with: cookies) {
                    request.setValue(value, forHTTPHeaderField: name)
                }
            }
            let (data, response) = try await ClientDiagnostics.data(for: request, using: transport)
            guard let http = response as? HTTPURLResponse else {
                throw NativeServerSetupError.server(
                    status: 0,
                    message: String(localized: "No HTTP response was returned.")
                )
            }
            if let url = request.url {
                let headerFields = http.allHeaderFields.reduce(into: [String: String]()) {
                    result, field in
                    result[String(describing: field.key)] = String(describing: field.value)
                }
                let cookies = HTTPCookie.cookies(withResponseHeaderFields: headerFields, for: url)
                cookieStorage?.setCookies(cookies, for: url, mainDocumentURL: nil)
            }
            guard (200..<300).contains(http.statusCode) else {
                let apiError = try? JSONCoding.decoder().decode(APIErrorPayload.self, from: data)
                let message = apiError.map { "\($0.code): \($0.message)" }
                    ?? String(decoding: data, as: UTF8.self)
                throw NativeServerSetupError.server(status: http.statusCode, message: message)
            }
            return try JSONCoding.decoder().decode(Response.self, from: data)
        }
    }

    private static func makeEphemeralTransport() -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.urlCache = nil
        configuration.httpShouldSetCookies = true
        configuration.httpCookieAcceptPolicy = .always
        configuration.timeoutIntervalForRequest = 8
        configuration.timeoutIntervalForResource = 60 * 6
        return URLSession(configuration: configuration)
    }
}
