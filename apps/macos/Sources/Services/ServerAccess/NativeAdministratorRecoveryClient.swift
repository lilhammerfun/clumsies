import Foundation

struct NativeAdministratorRecoverySnapshot: Sendable {
    let health: AdminHealthRecord
    let members: [AdminOrganizationMemberRecord]
    let tokens: [AdminAccessTokenRecord]
}

struct NativeAdministratorRecoveryClient: Sendable {
    let session: NativeAuthenticatedSession

    func load() async throws -> NativeAdministratorRecoverySnapshot {
        async let health = session.decode(
            AdminHealthRecord.self,
            path: "/api/v1/admin/health"
        )
        async let members = session.decode(
            ListResponse<AdminOrganizationMemberRecord>.self,
            path: "/api/v1/admin/members?limit=200"
        )
        async let tokens = session.decode(
            ListResponse<AdminAccessTokenRecord>.self,
            path: "/api/v1/admin/tokens?limit=200"
        )
        let loaded = try await (health, members, tokens)
        return .init(
            health: loaded.0,
            members: loaded.1.items,
            tokens: loaded.2.items
        )
    }

    func updateMember(
        _ member: AdminOrganizationMemberRecord,
        role: AdminOrganizationRole? = nil,
        status: AdminMemberStatus? = nil
    ) async throws -> AdminOrganizationMemberRecord {
        let body = try JSONCoding.encoder().encode(
            UpdateAdminOrganizationMemberRequest(role: role, status: status)
        )
        return try await session.decode(
            AdminOrganizationMemberRecord.self,
            path: "/api/v1/admin/members/\(member.id)",
            method: "PATCH",
            headers: ["If-Match": String(member.revision)],
            body: body
        )
    }

    func revokeToken(_ token: AdminAccessTokenRecord) async throws {
        let _: DeleteResult = try await session.decode(
            DeleteResult.self,
            path: "/api/v1/admin/tokens/\(token.id)",
            method: "DELETE",
            body: JSONCoding.encoder().encode(EmptyPayload())
        )
    }
}
