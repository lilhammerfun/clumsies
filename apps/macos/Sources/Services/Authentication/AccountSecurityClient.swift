import Foundation

struct AccountSecurityClient: Sendable {
    let daemon: DaemonXPCClient
    let server: ServerClient

    func changePassword(_ request: ChangeAccountPassword) async throws -> NativeAuthenticatedSession {
        let config = try await daemon.projectConfig()
        let tokens: TokenResponse = try await server.send(method: "POST", path: "/api/v1/auth/password", body: request)
        let origin = try ServerOrigin(validating: config.serverUrl)
        return try await AuthenticationClient(serverURL: origin.url).authenticatedSession(tokens: tokens)
    }

    func bindOIDC(currentPassword: String?) async throws -> NativeAuthenticatedSession {
        let config = try await daemon.projectConfig()
        let origin = try ServerOrigin(validating: config.serverUrl)
        let flow = try NativeBrowserAuthorizationFlow.start()
        defer { flow.cancel() }
        let parameters = flow.parameters
        let response: NativeSetupOIDCAuthorizationResponse = try await server.send(method: "POST", path: "/api/v1/auth/oidc-bindings", body: BindAccountOIDC(authorization: .init(redirectUri: parameters.redirectUri, state: parameters.state, codeChallenge: parameters.codeChallenge, codeChallengeMethod: parameters.codeChallengeMethod), currentPassword: currentPassword))
        guard let url = URL(string: response.authorizationUrl), url.scheme == "https" || (url.scheme == "http" && url.host.map(ServerOrigin.isLoopback) == true) else {
            throw AuthenticationError.invalidAuthorizationURL
        }
        let session = try await AuthenticationClient(serverURL: origin.url).authenticate(using: flow.authorize(at: url))
        return session
    }
}
