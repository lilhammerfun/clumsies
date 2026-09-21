import Foundation
import Combine

private enum NativeServerAccessError: UserFacingError {
    case missingOrganization
    case missingProject
    case missingSetupCode
    case daemonDidNotStart(String?)

    var errorDescription: String? {
        switch self {
        case .missingOrganization:
            String(localized: "Enter an organization name.")
        case .missingProject:
            String(localized: "Enter a default project name.")
        case .missingSetupCode:
            String(localized: "Enter the setup code from the Server deployment.")
        case .daemonDidNotStart(let detail):
            detail ?? String(localized: "The local daemon did not start.")
        }
    }
}

@MainActor
final class NativeServerAccessModel: ObservableObject {
    enum Purpose {
        case appSignIn
        case administratorRecovery
    }

    enum Destination {
        case daemon(DaemonXPCClient, launchIfNeeded: Bool)
        case memoryOnly
    }

    @Published var serverOrigin: String
    @Published var setupCode = ""
    @Published var organizationName = ""
    @Published var defaultProjectName = String(localized: "Default")
    @Published var allowedEmailDomains = ""
    @Published private(set) var showsSetup = false
    @Published private(set) var setupCodeConfigured = true
    @Published private(set) var oidcConfigured = true
    @Published private(set) var isBusy = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var recoveryReady = false

    let purpose: Purpose

    private let destination: Destination
    private let developmentInstanceID: String?
    let recoveryState: NativeAdministratorRecoveryState
    private let onCompleted: @MainActor () -> Void

    init(
        serverURL: URL = ClumsiesIdentifiers.serverURL,
        purpose: Purpose,
        destination: Destination,
        recoveryState: NativeAdministratorRecoveryState,
        initialSetupStatus: NativeSetupStatus? = nil,
        developmentInstanceID: String? = ClumsiesIdentifiers.developmentInstanceID,
        onCompleted: @escaping @MainActor () -> Void = {}
    ) {
        serverOrigin = serverURL.absoluteString
        self.purpose = purpose
        self.destination = destination
        self.developmentInstanceID = developmentInstanceID
        self.recoveryState = recoveryState
        self.onCompleted = onCompleted
        if let initialSetupStatus {
            apply(initialSetupStatus)
        }
    }

    var usesAutomaticDevelopmentLogin: Bool {
        purpose == .appSignIn && developmentInstanceID != nil
            && URL(string: serverOrigin)?.host.map(ServerOrigin.isLoopback) == true
    }

    var title: String {
        if usesAutomaticDevelopmentLogin { return String(localized: "Dev Instance is not ready") }
        if recoveryReady { return String(localized: "Recovery session ready") }
        if showsSetup { return String(localized: "Set up Clumsies Server") }
        return switch purpose {
        case .appSignIn: String(localized: "Sign in to Clumsies")
        case .administratorRecovery: String(localized: "Administrator recovery")
        }
    }

    var subtitle: String {
        if usesAutomaticDevelopmentLogin {
            return String(localized: "Run just dev-macos in this worktree to initialize the local Server and sign in automatically.")
        }
        if recoveryReady {
            return String(localized: "The administrator session is available in this App only and was not saved to disk.")
        }
        if showsSetup {
            return String(localized: "Create the first organization and owner without a Web console.")
        }
        return switch purpose {
        case .appSignIn:
            String(localized: "Connect to your Server, then continue in the system browser.")
        case .administratorRecovery:
            String(localized: "Sign in directly to the Server while the local daemon is unavailable.")
        }
    }

    var recoveryIdentity: String? {
        guard let session = recoveryState.session else { return nil }
        return "\(session.currentUser.user.email) · \(session.currentUser.org.name)"
    }

    func continueFromServer() {
        run {
            let origin = try ServerOrigin(validating: self.serverOrigin)
            let setup = NativeServerSetupClient(origin: origin)
            let status = try await setup.status()
            try self.persist(origin)
            if status.state == .setupRequired {
                self.apply(status)
                return
            }
            guard status.oidcConfigured else {
                throw NativeServerSetupError.oidcNotConfigured
            }
            let session = try await AuthenticationClient(serverURL: origin.url).authenticate()
            try await self.finish(session)
        }
    }

    func completeSetup() {
        run {
            let organizationName = self.organizationName
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !organizationName.isEmpty else {
                throw NativeServerAccessError.missingOrganization
            }
            let defaultProjectName = self.defaultProjectName
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !defaultProjectName.isEmpty else {
                throw NativeServerAccessError.missingProject
            }
            let setupCode = self.setupCode.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !setupCode.isEmpty else {
                throw NativeServerAccessError.missingSetupCode
            }

            let origin = try ServerOrigin(validating: self.serverOrigin)
            let setup = NativeServerSetupClient(origin: origin)
            let status = try await setup.status()
            self.apply(status)
            try self.persist(origin)
            let configuration = NativeSetupConfiguration(
                orgName: organizationName,
                defaultProjectName: defaultProjectName,
                allowedEmailDomains: Self.emailDomains(from: self.allowedEmailDomains)
            )
            let session = try await setup.completeSetup(
                setupCode: setupCode,
                configuration: configuration
            )
            try await self.finish(session)
        }
    }

    func chooseAnotherServer() {
        guard !isBusy else { return }
        showsSetup = false
        errorMessage = nil
    }

    private func apply(_ status: NativeSetupStatus) {
        showsSetup = status.state == .setupRequired && !usesAutomaticDevelopmentLogin
        setupCodeConfigured = status.setupCodeConfigured
        oidcConfigured = status.oidcConfigured
        if let configuration = status.session?.configuration {
            organizationName = configuration.orgName
            defaultProjectName = configuration.defaultProjectName
            allowedEmailDomains = configuration.allowedEmailDomains.joined(separator: ", ")
        }
    }

    private func finish(_ session: NativeAuthenticatedSession) async throws {
        recoveryState.retain(session)
        switch destination {
        case .memoryOnly:
            recoveryReady = true
        case .daemon(let daemon, let launchIfNeeded):
            if launchIfNeeded {
                let state = try await DaemonBootstrapController().ensureRunning()
                guard state.running else {
                    throw NativeServerAccessError.daemonDidNotStart(state.error)
                }
                _ = try await DaemonStartupReadiness().waitForHealth { timeout in
                    try await daemon.health(timeout: timeout)
                }
            }
            _ = try await session.install(on: daemon)
            recoveryState.clear()
            onCompleted()
        }
    }

    private func persist(_ origin: ServerOrigin) throws {
        _ = try ClumsiesIdentifiers.persistServerOrigin(origin.url.absoluteString)
        serverOrigin = origin.url.absoluteString
    }

    private func run(_ operation: @escaping @MainActor () async throws -> Void) {
        guard !isBusy else { return }
        isBusy = true
        errorMessage = nil
        Task { @MainActor in
            defer { isBusy = false }
            do {
                try await operation()
            } catch {
                errorMessage = error.actionMessage
                if recoveryState.isAuthenticated {
                    recoveryReady = true
                }
            }
        }
    }

    private static func emailDomains(from input: String) -> [String] {
        var seen = Set<String>()
        return input
            .components(separatedBy: CharacterSet(charactersIn: ",;\n\t "))
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
            .filter { !$0.isEmpty && seen.insert($0).inserted }
    }
}
