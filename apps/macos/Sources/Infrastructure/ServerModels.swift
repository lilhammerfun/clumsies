import Foundation

struct UserReference: Codable, Hashable, Sendable {
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

struct OrganizationMemberRecord: Codable, Identifiable, Hashable, Sendable {
    var id: String { userId }

    let userId: String
    let email: String
    let displayName: String?
    let role: String
    let status: String
    let externalIdentityBound: Bool
    let revision: Int
}

struct ProjectMemberRecord: Codable, Identifiable, Hashable, Sendable {
    var id: String { user.userId }

    let projectId: String
    let user: UserReference
    let role: ProjectMemberRole
    let joinedAt: String
}

struct CreateOrganizationMemberRequest: Codable, Sendable {
    let email: String
    let role: String
}

struct CreateProjectMemberRequest: Codable, Sendable {
    let userId: String
    let role: ProjectMemberRole
}

struct AssignKanbanIssueRequest: Codable, Sendable {
    let assigneeUserId: String
}

struct KanbanIssueAssignmentResponse: Codable, Sendable {
    let assignee: UserReference
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

    var hasDraftResultChanges: Bool {
        draftState != (proposedState ?? draftState)
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
    let candidateId: String?
    let resolvedState: ReconciliationResourceState?
}

struct CreateReviewSubmissionRequest: Codable, Sendable {
    let expectedReviewVersion: Int
    let drafts: [ReviewDraftRequest]
    let title: String
    let description: String
    let candidateId: String?
    let resolvedState: ReconciliationResourceState?
}

struct ReviewDraftRequest: Codable, Sendable {
    let draftId: String
    let expectedDraftVersion: Int
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

enum ProjectEpisodeStatus: String, Codable, Sendable {
    case pendingSummary = "pending_summary"
    case active
    case noMemory = "no_memory"
    case deleted
}

struct ProjectEpisodeSummary: Codable, Hashable, Sendable {
    let revision: Int
    let summaryAlgorithmRevision: String
    let policyRevision: Int
    let evidenceHash: String
    let body: String
    let noMemory: Bool
    let createdAt: String
}

struct ProjectEpisode: Codable, Identifiable, Hashable, Sendable {
    var id: String { episodeId }

    let episodeId: String
    let projectId: String
    let runId: String
    let hostSessionId: String?
    let host: String
    let evidenceFormat: String
    let evidenceFormatRevision: Int
    let activityAt: String
    let evidenceHash: String
    let evidenceBytes: Int
    let status: ProjectEpisodeStatus
    let currentSummary: ProjectEpisodeSummary?
    let revision: Int
    let corpusRevision: Int
    let createdAt: String
    let updatedAt: String
    let deletedAt: String?
}

struct ProjectEpisodeListResponse: Codable, Sendable {
    let items: [ProjectEpisode]
    let corpusRevision: Int
    let nextCursor: Int?
    let hasMore: Bool
}

struct ProjectEpisodeSummaryPolicy: Codable, Sendable {
    let projectId: String
    let instructions: String
    let revision: Int
    let updatedAt: String
}

struct UpdateProjectEpisodeSummaryPolicyRequest: Codable, Sendable {
    let instructions: String
}

struct ProjectEpisodeSummaryPreviewRequest: Codable, Sendable {
    let instructions: String?
}

struct ProjectEpisodeSummaryPreview: Codable, Identifiable, Sendable {
    var id: String { episodeId }

    let episodeId: String
    let evidenceHash: String
    let summaryAlgorithmRevision: String
    let policyRevision: Int
    let body: String
    let noMemory: Bool
}

struct ProjectEpisodeEvidenceItem: Codable, Identifiable, Sendable {
    var id: String { "\(sequence):\(byteOffset)" }

    let sequence: Int
    let occurredAt: String
    let kind: String
    let byteOffset: Int
    let content: String
    let complete: Bool
}

struct ProjectEpisodeEvidencePage: Codable, Sendable {
    let episodeId: String
    let projectId: String
    let runId: String
    let evidenceHash: String
    let items: [ProjectEpisodeEvidenceItem]
    let nextCursor: String?
    let hasMore: Bool
    let untrusted: Bool
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
