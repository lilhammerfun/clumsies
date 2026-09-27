import Foundation

struct NativeLoginMethods: Decodable, Sendable {
    let passwordEnabled: Bool
    let oidcEnabled: Bool
    let google: Bool
}

struct NativePasswordLogin: Encodable, Sendable {
    let username: String
    let password: String
}

struct NativeRedeemAction: Encodable, Sendable {
    let token: String
    let username: String?
    let password: String
}

struct AccountActionCredential: Decodable, Identifiable, Sendable {
    var id: String { tokenId }
    let tokenId: String
    let userId: String
    let token: String
    let expiresAt: String
}

struct AccountCredentialStatus: Decodable, Sendable {
    let username: String?
    let passwordSet: Bool
    let oidcEmail: String?
}

struct ChangeAccountPassword: Encodable, Sendable {
    let username: String?
    let currentPassword: String?
    let password: String
}

struct CreateMemberInvitation: Encodable, Sendable {
    let role: AdminOrganizationRole
}

struct BindAccountOIDC: Encodable, Sendable {
    let authorization: Authorization
    let currentPassword: String?

    struct Authorization: Encodable, Sendable {
        let clientKind = "desktop"
        let redirectUri: String
        let state: String
        let codeChallenge: String
        let codeChallengeMethod: String


    }
}
