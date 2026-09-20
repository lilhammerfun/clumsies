import Foundation

struct UserReference: Codable, Identifiable, Hashable, Sendable {
    var id: String { userId }
    let userId: String
    let email: String
    let displayName: String?
    let avatarUrl: String?
    let role: String
}

struct OrganizationReference: Codable, Hashable, Sendable {
    let orgId: String
    let name: String
}

struct ProjectReference: Codable, Identifiable, Hashable, Sendable {
    var id: String { projectId }

    let projectId: String
    let name: String
    var role: ProjectMemberRole? = nil
}

struct ProjectRecord: Codable, Identifiable, Hashable, Sendable {
    var id: String { projectId }

    let projectId: String
    let name: String
    let description: String
    let revision: Int
    let createdAt: String
    let updatedAt: String
}

struct CreateProjectRequest: Codable, Sendable {
    let name: String
    let description: String?
}

struct UpdateProjectRequest: Codable, Sendable {
    let name: String?
    let description: String?
}

struct CurrentUserResponse: Codable, Sendable {
    let user: UserReference
    let org: OrganizationReference
    let projects: [ProjectReference]
    let defaultProjectId: String?
    let capabilities: [String]
}

struct PageInfo: Codable, Sendable {
    let nextCursor: String?
    let hasMore: Bool
}

enum ProjectMemberRole: String, Codable, CaseIterable, Sendable {
    case member
    case admin

    var title: String {
        switch self {
        case .member: "Member"
        case .admin: "Admin"
        }
    }
}

struct ProjectMemberRecord: Codable, Identifiable, Hashable, Sendable {
    var id: String { user.userId }

    let projectId: String
    let user: UserReference
    let role: ProjectMemberRole
    let joinedAt: String
}

struct CreateProjectMemberRequest: Codable, Sendable {
    let userId: String
    let role: ProjectMemberRole
}

enum AdminOrganizationRole: String, Codable, CaseIterable, Identifiable, Sendable {
    case owner
    case admin
    case member

    var id: String { rawValue }

    var title: String {
        switch self {
        case .owner: "Owner"
        case .admin: "Admin"
        case .member: "Member"
        }
    }
}

enum AdminMemberStatus: String, Codable, CaseIterable, Identifiable, Sendable {
    case invited
    case active
    case disabled

    var id: String { rawValue }

    var title: String {
        switch self {
        case .invited: "Not signed in yet"
        case .active: "Active"
        case .disabled: "Disabled"
        }
    }
}

struct AdminOrganizationRecord: Codable, Hashable, Sendable {
    let orgId: String
    let name: String
    let allowedEmailDomains: [String]
    let revision: Int
    let updatedAt: String
}

struct AdminOrganizationMemberRecord: Codable, Identifiable, Hashable, Sendable {
    var id: String { userId }

    let userId: String
    let email: String
    let displayName: String?
    let role: AdminOrganizationRole
    let status: AdminMemberStatus
    let externalIdentityBound: Bool
    let revision: Int
}

struct AdminProjectRecord: Codable, Identifiable, Hashable, Sendable {
    var id: String { projectId }

    let projectId: String
    let name: String
    let description: String
    let memberCount: Int
    let revision: Int
    let createdAt: String
    let updatedAt: String
}

enum AdminAccessTokenKind: String, Codable, CaseIterable, Sendable {
    case access
    case refresh
    case integration
    case webSession = "web_session"

    var title: String {
        switch self {
        case .access: "Access"
        case .refresh: "Refresh"
        case .integration: "Integration"
        case .webSession: "Web session"
        }
    }
}

struct AdminAccessTokenRecord: Codable, Identifiable, Hashable, Sendable {
    var id: String { tokenId }

    let tokenId: String
    let userId: String
    let kind: AdminAccessTokenKind
    let revoked: Bool
    let expiresAt: String?
    let createdAt: String
}

struct AdminAuditEventRecord: Codable, Identifiable, Hashable, Sendable {
    var id: String { eventId }

    let eventId: String
    let actorUserId: String?
    let actorDisplayName: String?
    let actorEmail: String?
    let action: String
    let targetType: String
    let targetId: String?
    let targetDisplayName: String?
    let createdAt: String
}

struct AdminIdentityProviderStatus: Codable, Hashable, Sendable {
    let `protocol`: String
    let configured: Bool
    let issuer: String?
    let callbackUrl: String?
    let admissionMode: String
    let secretSource: String
}

enum AdminHealthStatus: String, Codable, Sendable {
    case ok
    case degraded
    case down

    var title: String { rawValue.capitalized }
}

struct AdminHealthCheck: Codable, Hashable, Sendable {
    let status: AdminHealthStatus
    let message: String
}

struct AdminHealthRecord: Codable, Hashable, Sendable {
    let status: AdminHealthStatus
    let version: String
    let database: AdminHealthCheck
    let schema: AdminHealthCheck
    let commitService: AdminHealthCheck
    let oidc: AdminHealthCheck
}

struct UpdateAdminOrganizationRequest: Codable, Sendable {
    let name: String?
    let allowedEmailDomains: [String]?
}

struct CreateAdminOrganizationMemberRequest: Codable, Sendable {
    let email: String
    let role: AdminOrganizationRole
}

struct UpdateAdminOrganizationMemberRequest: Codable, Sendable {
    let role: AdminOrganizationRole?
    let status: AdminMemberStatus?
}

struct ListResponse<Item: Decodable & Sendable>: Decodable, Sendable {
    let items: [Item]
    let pageInfo: PageInfo
}

struct MemoryMetadata: Codable, Identifiable, Hashable, Sendable {
    var id: String { memoryId }

    let memoryId: String
    let scope: String
    let projectId: String?
    let path: String
    let name: String
    let description: String
    let contentHash: String
    let status: String
    let updatedAt: String
}

struct MemoryDetail: Codable, Sendable {
    let memory: MemoryMetadata
    let content: String
    let etag: String
}

struct PersonalBundleMetadata: Codable, Identifiable, Hashable, Sendable {
    var id: String { bundleId }

    let bundleId: String
    let ownerUserId: String
    let name: String
    let description: String
    let resourceCount: Int
    let revision: Int
    let createdAt: String
    let updatedAt: String
}

struct PersonalBundleDetail: Codable, Sendable {
    let bundle: PersonalBundleMetadata
    let memories: [MemoryMetadata]
    let etag: String
}

struct PersonalBundleRequest: Codable, Sendable {
    let name: String
    let description: String
    let resourceIds: [String]
}

struct ProjectOrgSelection: Codable, Sendable {
    let projectId: String
    let memories: [MemoryMetadata]
    let revision: Int
}

struct CommitStateResponse: Codable, Sendable {
    let updateAvailable: Bool
    let ref: CommitReference
    let latest: CommitMetadata?
    let downloadUrl: String?
    let incrementalSupported: Bool
}

struct CommitReference: Codable, Sendable {
    let name: String
    let scope: String
    let orgId: String
    let projectId: String?
    let commitId: String?
    let updatedAt: String
}

struct CommitMetadata: Codable, Sendable {
    let commitId: String
    let scope: String
    let orgId: String
    let projectId: String?
    let treeId: String
    let parentCommitId: String?
    let version: Int
    let createdAt: String
}

struct CommitPayload: Codable, Sendable {
    let commit: CommitMetadata
    let tree: CommitTree
    let blobs: [CommitBlob]
}

struct CommitTree: Codable, Sendable {
    let treeId: String
    let entries: [CommitTreeEntry]
}

enum ServerTreeEntryKind: String, Codable, Hashable, Sendable {
    case memory
    case projectOrgSelection = "project_org_selection"

    /// Tree entries are Memory (or the internal project-org-selection entry).
    /// Legacy context/rule/workflow entry types from archived commits decode
    /// to `.memory`.
    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        switch raw {
        case "context", "rule", "workflow", "memory":
            self = .memory
        case "project_org_selection":
            self = .projectOrgSelection
        default:
            throw DecodingError.dataCorrupted(.init(
                codingPath: decoder.codingPath,
                debugDescription: "Unknown server tree entry kind: \(raw)"
            ))
        }
    }
}

struct CommitTreeEntry: Codable, Sendable {
    let id: String
    let type: ServerTreeEntryKind
    let scope: String
    let projectId: String?
    let path: String?
    let blobId: String
    let source: String
    let description: String?
}

struct CommitBlob: Codable, Sendable {
    let blobId: String
    let content: String
}

struct ReviewMetadata: Codable, Identifiable, Hashable, Sendable {
    var id: String { reviewId }

    let reviewId: String
    let projectId: String
    let draftId: String
    var draftIds: [String]? = nil
    let author: UserReference
    let title: String
    let description: String
    let status: String
    let version: Int
    let decisionBody: String?
    let approvedResultHash: String?
    let decidedBy: UserReference?
    let decidedAt: String?
    let coordination: DraftCoordination
    let createdAt: String
    let updatedAt: String
}

struct ReviewComment: Codable, Identifiable, Hashable, Sendable {
    var id: String { commentId }

    let commentId: String
    let reviewId: String
    let author: UserReference
    let body: String
    let createdAt: String
    let anchorPath: String?
    let anchorLine: Int?
    let reviewVersion: Int
}

struct ServerDraftResourceReference: Codable, Hashable, Sendable {
    let scope: String
    let id: String?
    let path: String?
}

struct ServerDraft: Codable, Sendable {
    let draftId: String
    let projectId: String
    let baseCommitId: String?
    let author: UserReference
    let title: String
    let description: String
    let resource: ServerDraftResourceReference
    let status: String
    let coordination: DraftCoordination
    let version: Int
    let createdAt: String
    let updatedAt: String
}

struct ServerDraftOperation: Codable, Identifiable, Sendable {
    var id: String { operationId }

    let action: String
    let resource: ServerDraftResourceReference
    let content: DaemonDraftContent?
    let newPath: String?
    let operationId: String
    let createdAt: String
}

struct ReviewDetail: Codable, Sendable {
    let review: ReviewMetadata
    let draft: ServerDraft
    let operations: [ServerDraftOperation]
    var drafts: [ReviewDraftDetail]? = nil
    let comments: [ReviewComment]
}

struct ReviewDraftDetail: Codable, Sendable {
    let draft: ServerDraft
    let operations: [ServerDraftOperation]
}

struct DraftCoordination: Codable, Hashable, Sendable {
    let freshness: DraftFreshness
    let currentCommitId: String?
    let hasUpstreamResourceChanges: Bool
    let reconciliation: DraftReconciliationStatus
    let candidateId: String?
}

struct ReconciliationResourceState: Codable, Hashable, Sendable {
    let exists: Bool
    let resource: ServerDraftResourceReference
    let content: DaemonDraftContent?
}

struct ReconciliationConflict: Codable, Hashable, Sendable {
    let kind: String
    let field: String
    let base: String?
    let current: String?
    let draft: String?
}

struct DraftReconciliationCandidate: Codable, Identifiable, Hashable, Sendable {
    var id: String { candidateId }

    let candidateId: String
    let draftId: String
    let draftVersion: Int
    let baseCommitId: String?
    let currentCommitId: String?
    let status: DraftReconciliationStatus
    let baseState: ReconciliationResourceState
    let currentState: ReconciliationResourceState
    let draftState: ReconciliationResourceState
    let proposedState: ReconciliationResourceState?
    let conflicts: [ReconciliationConflict]
    let resultHash: String?
    let valid: Bool
    let createdAt: String
    let invalidatedAt: String?

    var postSyncDiffStates: (base: ReconciliationResourceState, draft: ReconciliationResourceState) {
        (currentState, proposedState ?? draftState)
    }
}

struct CreateDraftReconciliationCandidateRequest: Codable, Sendable {
    let expectedDraftVersion: Int
}

struct CreateDraftRebaseRequest: Codable, Sendable {
    let candidateId: String
    let expectedDraftVersion: Int
    let resolvedState: ReconciliationResourceState?
}

struct DraftRebaseResult: Codable, Sendable {
    let rebaseId: String
    let previousRevisionId: String
    let draft: ServerDraftDetail
    let review: ReviewMetadata?
    let approvalInvalidated: Bool
}

struct ServerDraftDetail: Codable, Sendable {
    let draft: ServerDraft
    let operations: [ServerDraftOperation]
    let syncState: ServerDraftSyncState
}

struct ServerDraftSyncState: Codable, Sendable {
    let status: String
    let serverCursor: String?
    let daemonInstallationId: String?
}

struct CreateReviewRequest: Codable, Sendable {
    let drafts: [ReviewDraftRequest]
    let title: String
    let description: String
}

struct CreateReviewSubmissionRequest: Codable, Sendable {
    let expectedReviewVersion: Int
    let drafts: [ReviewDraftRequest]
    let title: String
    let description: String
}

struct ReviewDraftRequest: Codable, Sendable {
    let draftId: String
    let expectedDraftVersion: Int
    let candidateId: String?
    let resolvedState: ReconciliationResourceState?
}

struct CreateReviewUpdatePlanRequest: Codable, Sendable {
    let expectedReviewVersion: Int
}

struct ReviewUpdatePlan: Codable, Sendable {
    let detail: ReviewDetail
    let candidates: [DraftReconciliationCandidate]
    var contentMerges: [String: ReviewConflictContent] = [:]
}

struct ReviewConflictContent: Codable, Sendable {
    let text: String
    let markerLength: Int
}

struct CreateReviewUpdateRequest: Codable, Sendable {
    let expectedReviewVersion: Int
    let drafts: [ReviewDraftRequest]
}

struct ReviewDraftReconciliation: Sendable {
    let candidate: DraftReconciliationCandidate
    let resolvedState: ReconciliationResourceState?
}

struct CreateReviewCommentRequest: Codable, Sendable {
    let body: String
    let expectedReviewVersion: Int
    let anchorPath: String?
    let anchorLine: Int?

    init(
        body: String,
        expectedReviewVersion: Int,
        anchorPath: String? = nil,
        anchorLine: Int? = nil
    ) {
        self.body = body
        self.expectedReviewVersion = expectedReviewVersion
        self.anchorPath = anchorPath
        self.anchorLine = anchorLine
    }
}

struct CreateReviewDecisionRequest: Codable, Sendable {
    let decision: String
    let expectedReviewVersion: Int
    let body: String
}

struct CreateReviewMergeRequest: Codable, Sendable {
    let expectedReviewVersion: Int
}

struct DraftOperationInput: Codable, Sendable {
    let action: String
    let resource: ServerDraftResourceReference
    let content: DaemonDraftContent?
    let newPath: String?
}

struct ReviewMergeResponse: Codable, Sendable {
    let review: ReviewMetadata
    let commitId: String?
    let appliedOperationCount: Int
}

struct ReplaceProjectOrgSelectionRequest: Codable, Sendable {
    let resourceIds: [String]
}

struct DeleteResult: Codable, Sendable {
    let deleted: Bool
    let id: String
}

struct TokenResponse: Codable, Sendable {
    let accessToken: String
    let refreshToken: String
}

struct TokenExchangeRequest: Encodable, Sendable {
    let grantType = "authorization_code"
    let code: String
    let redirectUri: String
    let codeVerifier: String
}
