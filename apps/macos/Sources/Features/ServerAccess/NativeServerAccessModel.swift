import Foundation
import Combine

@MainActor
final class NativeAdministratorRecoveryState: ObservableObject {
    @Published private(set) var session: NativeAuthenticatedSession?
    @Published private(set) var health: AdminHealthRecord?
    @Published private(set) var members: [AdminOrganizationMemberRecord] = []
    @Published private(set) var tokens: [AdminAccessTokenRecord] = []
    @Published private(set) var isLoading = false
    @Published private(set) var mutatingID: String?
    @Published private(set) var errorMessage: String?

    var isAuthenticated: Bool { session != nil }
    var currentUserID: String? { session?.currentUser.user.userId }

    func retain(_ session: NativeAuthenticatedSession) {
        self.session = session
        health = nil
        members = []
        tokens = []
        errorMessage = nil
    }

    func clear() {
        session = nil
        health = nil
        members = []
        tokens = []
        errorMessage = nil
    }

    func load() async {
        guard let session, !isLoading else { return }
        isLoading = true
        errorMessage = nil
        defer { isLoading = false }
        do {
            let snapshot = try await NativeAdministratorRecoveryClient(session: session).load()
            health = snapshot.health
            members = snapshot.members
            tokens = snapshot.tokens
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func setRole(
        _ role: AdminOrganizationRole,
        for member: AdminOrganizationMemberRecord
    ) async {
        await update(member, role: role, status: nil)
    }

    func setDisabled(_ disabled: Bool, for member: AdminOrganizationMemberRecord) async {
        await update(member, role: nil, status: disabled ? .disabled : .active)
    }

    func revoke(_ token: AdminAccessTokenRecord) async {
        guard let session, mutatingID == nil else { return }
        mutatingID = token.id
        errorMessage = nil
        defer { mutatingID = nil }
        do {
            try await NativeAdministratorRecoveryClient(session: session).revokeToken(token)
            tokens.removeAll { $0.id == token.id }
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func update(
        _ member: AdminOrganizationMemberRecord,
        role: AdminOrganizationRole?,
        status: AdminMemberStatus?
    ) async {
        guard let session, mutatingID == nil else { return }
        mutatingID = member.id
        errorMessage = nil
        defer { mutatingID = nil }
        do {
            let updated = try await NativeAdministratorRecoveryClient(session: session)
                .updateMember(member, role: role, status: status)
            if let index = members.firstIndex(where: { $0.id == updated.id }) {
                members[index] = updated
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

private enum NativeServerAccessError: LocalizedError {
    case missingOrganization
    case missingProject
    case missingSetupCode
    case daemonDidNotStart(String?)

    var errorDescription: String? {
        switch self {
        case .missingOrganization:
            "Enter an organization name."
        case .missingProject:
            "Enter a default project name."
        case .missingSetupCode:
            "Enter the setup code from the Server deployment."
        case .daemonDidNotStart(let detail):
            detail ?? "The local daemon did not start."
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
    @Published var defaultProjectName = "Default"
    @Published var allowedEmailDomains = ""
    @Published private(set) var showsSetup = false
    @Published private(set) var setupCodeConfigured = true
    @Published private(set) var oidcConfigured = true
    @Published private(set) var isBusy = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var recoveryReady = false

    let purpose: Purpose

    private let destination: Destination
    let recoveryState: NativeAdministratorRecoveryState
    private let onCompleted: @MainActor () -> Void

    init(
        serverURL: URL = ClumsiesIdentifiers.serverURL,
        purpose: Purpose,
        destination: Destination,
        recoveryState: NativeAdministratorRecoveryState,
        initialSetupStatus: NativeSetupStatus? = nil,
        onCompleted: @escaping @MainActor () -> Void = {}
    ) {
        serverOrigin = serverURL.absoluteString
        self.purpose = purpose
        self.destination = destination
        self.recoveryState = recoveryState
        self.onCompleted = onCompleted
        if let initialSetupStatus {
            apply(initialSetupStatus)
        }
    }

    var title: String {
        if recoveryReady { return "Recovery session ready" }
        if showsSetup { return "Set up Clumsies Server" }
        return switch purpose {
        case .appSignIn: "Sign in to Clumsies"
        case .administratorRecovery: "Administrator recovery"
        }
    }

    var subtitle: String {
        if recoveryReady {
            return "The administrator session is available in this App only and was not saved to disk."
        }
        if showsSetup {
            return "Create the first organization and owner without a Web console."
        }
        return switch purpose {
        case .appSignIn:
            "Connect to your Server, then continue in the system browser."
        case .administratorRecovery:
            "Sign in directly to the Server while the local daemon is unavailable."
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
        showsSetup = status.state == .setupRequired
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
                errorMessage = error.localizedDescription
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
