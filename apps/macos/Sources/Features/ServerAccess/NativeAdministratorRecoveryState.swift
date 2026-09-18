import Combine
import Foundation

@MainActor
final class NativeAdministratorRecoveryState: ObservableObject {
    @Published private(set) var session: NativeAuthenticatedSession?
    @Published private(set) var health: AdminHealthRecord?
    @Published private(set) var members: [AdminOrganizationMemberRecord] = []
    @Published private(set) var tokens: [AdminAccessTokenRecord] = []
    @Published private(set) var isLoading = false
    @Published private(set) var mutatingID: String?
    @Published private(set) var errorMessage: String?

    private var generation = UUID()
    private let fetchSnapshot: (NativeAuthenticatedSession) async throws -> NativeAdministratorRecoverySnapshot

    init(fetchSnapshot: @escaping (NativeAuthenticatedSession) async throws -> NativeAdministratorRecoverySnapshot = {
        try await NativeAdministratorRecoveryClient(session: $0).load()
    }) { self.fetchSnapshot = fetchSnapshot }

    var isAuthenticated: Bool { session != nil }
    var currentUserID: String? { session?.currentUser.user.userId }

    func retain(_ session: NativeAuthenticatedSession) {
        clear()
        self.session = session
    }

    func clear() {
        generation = UUID()
        isLoading = false
        mutatingID = nil
        session = nil
        health = nil
        members = []
        tokens = []
        errorMessage = nil
    }

    func load() async {
        guard let session, !isLoading else { return }
        let requestGeneration = generation
        isLoading = true
        errorMessage = nil
        defer { if generation == requestGeneration { isLoading = false } }
        do {
            let snapshot = try await fetchSnapshot(session)
            guard generation == requestGeneration, !Task.isCancelled else { return }
            health = snapshot.health
            members = snapshot.members
            tokens = snapshot.tokens
        } catch {
            guard generation == requestGeneration, !Task.isCancelled else { return }
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
        let requestGeneration = generation
        mutatingID = token.id
        errorMessage = nil
        defer { if generation == requestGeneration { mutatingID = nil } }
        do {
            try await NativeAdministratorRecoveryClient(session: session).revokeToken(token)
            guard generation == requestGeneration, !Task.isCancelled else { return }
            tokens.removeAll { $0.id == token.id }
        } catch {
            guard generation == requestGeneration, !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }

    private func update(
        _ member: AdminOrganizationMemberRecord,
        role: AdminOrganizationRole?,
        status: AdminMemberStatus?
    ) async {
        guard let session, mutatingID == nil else { return }
        let requestGeneration = generation
        mutatingID = member.id
        errorMessage = nil
        defer { if generation == requestGeneration { mutatingID = nil } }
        do {
            let updated = try await NativeAdministratorRecoveryClient(session: session)
                .updateMember(member, role: role, status: status)
            guard generation == requestGeneration, !Task.isCancelled else { return }
            if let index = members.firstIndex(where: { $0.id == updated.id }) {
                members[index] = updated
            }
        } catch {
            guard generation == requestGeneration, !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }
}
