import Foundation

struct DaemonProjectConfig: Codable, Equatable, Sendable {
    let serverUrl: String
    let projectId: String?
    let memoryGuidelinesPath: String?
    let hasAccessToken: Bool
    let hasRefreshToken: Bool
    let ready: Bool
    let missingFields: [String]

    init(
        serverUrl: String,
        projectId: String? = nil,
        memoryGuidelinesPath: String? = nil,
        hasAccessToken: Bool,
        hasRefreshToken: Bool,
        ready: Bool,
        missingFields: [String] = []
    ) {
        self.serverUrl = serverUrl
        self.projectId = projectId
        self.memoryGuidelinesPath = memoryGuidelinesPath
        self.hasAccessToken = hasAccessToken
        self.hasRefreshToken = hasRefreshToken
        self.ready = ready
        self.missingFields = missingFields
    }
}

struct DaemonProjectConfigUpdate: Codable, Sendable {
    let serverUrl: String
    let projectId: String?
    let memoryGuidelinesPath: String?
    let accessToken: String?
    let refreshToken: String?

    init(
        serverUrl: String,
        projectId: String? = nil,
        memoryGuidelinesPath: String? = nil,
        accessToken: String? = nil,
        refreshToken: String? = nil
    ) {
        self.serverUrl = serverUrl
        self.projectId = projectId
        self.memoryGuidelinesPath = memoryGuidelinesPath
        self.accessToken = accessToken
        self.refreshToken = refreshToken
    }
}

struct DaemonProjectSelection: Codable, Sendable {
    let projectId: String
}

struct DaemonProjectBindingListRequest: Codable, Sendable {
    let projectId: String
}

struct DaemonProjectBindingReplaceRequest: Codable, Sendable {
    let workspaceRoot: String
    let projectId: String
    let expectedRevision: Int?
}

struct DaemonProjectBindingRemoveRequest: Codable, Sendable {
    let workspaceRoot: String
    let expectedRevision: Int
}

struct DaemonProjectBindingRemoveResponse: Codable, Sendable {
    let workspaceRoot: String
    let removed: Bool
}

struct DaemonProjectBinding: Codable, Identifiable, Equatable, Sendable {
    var id: String { workspaceRoot }

    let serverUrl: String
    let workspaceRoot: String
    let projectId: String
    let revision: Int
    let createdAt: String
    let updatedAt: String
}

struct DaemonProjectBindingListResponse: Codable, Sendable {
    let items: [DaemonProjectBinding]
}

enum ProjectAgentAdapterKind: String, Codable, CaseIterable, Hashable, Identifiable, Sendable {
    case codex
    case claudeCode = "claude-code"
    case opencode
    case dsh
    case antigravity

    var id: String { rawValue }

    var title: String {
        switch self {
        case .codex: "Codex"
        case .claudeCode: "Claude Code"
        case .opencode: "opencode"
        case .dsh: "DeepSeek Harness (dsh)"
        case .antigravity: "Antigravity"
        }
    }
}

struct DaemonAgentAdapterSetting: Codable, Identifiable, Equatable, Sendable {
    var id: ProjectAgentAdapterKind { adapter }
    let adapter: ProjectAgentAdapterKind
    let enabled: Bool
    let configured: Bool
    let installed: Bool
    let legacyRepositories: Int
}

struct DaemonAgentAdapterSettings: Codable, Sendable {
    let items: [DaemonAgentAdapterSetting]
}

struct DaemonSetAgentAdapterRequest: Codable, Sendable {
    let adapter: ProjectAgentAdapterKind
    let enabled: Bool
    let runtimeBinaryPath: String
    let hostBinaryPath: String?
}

struct DaemonCodexPluginRequest: Codable, Sendable {
    let runtimeBinaryPath: String
    let hostBinaryPath: String?
}

struct DaemonCodexPluginStatus: Codable, Equatable, Sendable {
    let hostInstalled: Bool
    let marketplaceInstalled: Bool
    let marketplaceConflict: Bool
    let pluginInstalled: Bool
    let pluginEnabled: Bool
    let installedVersion: String?
    let expectedVersion: String
    let ready: Bool
}

enum ProjectAgentAdapterDelivery: String, Codable, Sendable {
    case legacyFiles = "legacy_files"
    case hostPlugin = "host_plugin"
}

struct DaemonProjectAgentAdapterListRequest: Codable, Sendable {
    let projectId: String
}

struct DaemonProjectAgentAdapterInstallRequest: Codable, Sendable {
    let projectId: String
    let workspaceRoot: String
    let adapter: ProjectAgentAdapterKind
    let runtimeBinaryPath: String
    let hostBinaryPath: String?
    let expectedRevision: Int?
}

struct DaemonProjectAgentAdapterRemoveRequest: Codable, Sendable {
    let workspaceRoot: String
    let adapter: ProjectAgentAdapterKind
    let expectedRevision: Int
}

struct DaemonProjectAgentAdapterRemoveResponse: Codable, Sendable {
    let workspaceRoot: String
    let adapter: ProjectAgentAdapterKind
    let removed: Bool
}

struct DaemonProjectAgentAdapter: Codable, Identifiable, Equatable, Sendable {
    var id: String { "\(workspaceRoot):\(adapter.rawValue)" }

    let serverUrl: String
    let projectId: String
    let workspaceRoot: String
    let adapter: ProjectAgentAdapterKind
    let delivery: ProjectAgentAdapterDelivery
    let revision: Int
    let managedFiles: [String]
    let createdAt: String
    let updatedAt: String
}

struct DaemonProjectAgentAdapterListResponse: Codable, Sendable {
    let items: [DaemonProjectAgentAdapter]
}

struct DaemonLegacyAgentAdapterInspectionRequest: Codable, Sendable {
    let runtimeBinaryPath: String
}

struct DaemonLegacyAgentAdapterConflict: Codable, Equatable, Sendable {
    let installId: String
    let adapter: ProjectAgentAdapterKind
    let scope: String
    let targetRoot: String
    let code: String
    let message: String
}

struct DaemonLegacyAgentAdapterInspectionResponse: Codable, Equatable, Sendable {
    let scanned: Int
    let deferred: Int
    let conflicts: [DaemonLegacyAgentAdapterConflict]
}

enum DaemonProjectStorageMode: String, Codable, Sendable {
    case standard = "default"
    case custom
}

enum DaemonProjectStorageAvailability: String, Codable, Sendable {
    case ready
    case moving
    case unavailable
}

enum DaemonProjectStorageMoveState: String, Codable, Sendable {
    case preparing
    case materializing
    case verifying
    case switching
    case cleaning
    case completed
    case failed

    var isTerminal: Bool {
        self == .completed || self == .failed
    }
}

struct DaemonProjectStorageRequest: Codable, Sendable {
    let projectId: String
}

struct DaemonProjectStorageMoveRequest: Codable, Sendable {
    let moveId: String
}

struct DaemonProjectStorageReplaceRequest: Codable, Sendable {
    let projectId: String
    let selectedRootPath: String
    let handoffBookmarkData: String
    let expectedLocationRevision: Int
}

struct DaemonProjectStorageResetRequest: Codable, Sendable {
    let projectId: String
    let expectedLocationRevision: Int
}

struct DaemonProjectCacheClearRequest: Codable, Sendable {
    let projectId: String
    let expectedLocationRevision: Int
}

struct DaemonProjectStorage: Codable, Equatable, Sendable {
    let authorityKey: String
    let projectId: String
    let mode: DaemonProjectStorageMode
    let selectedRootPath: String
    let managedRootPath: String
    let activeGenerationPath: String?
    let searchIndexPath: String
    let availability: DaemonProjectStorageAvailability
    let locationRevision: Int
    let sizeBytes: UInt64
    let activeMoveId: String?
    let issueCode: String?
    let diagnostic: String?
}

struct DaemonProjectStorageMove: Codable, Equatable, Sendable {
    let moveId: String
    let projectId: String
    let sourceMode: DaemonProjectStorageMode
    let destinationMode: DaemonProjectStorageMode
    let sourceManagedRootPath: String
    let destinationManagedRootPath: String
    let sourceLocationRevision: Int
    let state: DaemonProjectStorageMoveState
    let errorCode: String?
    let errorMessage: String?
    let createdAt: String
    let updatedAt: String
    let completedAt: String?
}

struct DaemonProjectCheckoutRequest: Codable, Sendable {
    let projectId: String
}

struct DaemonProjectCheckout: Codable, Sendable {
    let projectId: String
    let commitId: String?
    let refEtag: String?
    let commitCreatedAt: String?
    let orgSelectionRevision: Int
    let selectedOrgResourceIds: [String]
    let resources: [DaemonProjectCheckoutResource]
    let ready: Bool
}

struct DaemonProjectCheckoutResource: Codable, Sendable {
    let resourceId: String
    let scope: DaemonDraftScope
    let resourceKind: DaemonResourceKind
    let projectId: String?
    let path: String
    let contentHash: String
    let content: DaemonDraftContent
}

struct DaemonHealth: Codable, Equatable, Sendable {
    let daemonVersion: String
    let agentRuntime: AgentRuntimeIdentity
    let serverUrl: String
    let projectId: String?
    let daemonInstallationId: String
    let logDir: String
    let localDb: DaemonLocalDatabaseStatus
}

struct AgentRuntimeIdentity: Codable, Equatable, Sendable {
    let protocolRevision: Int
    let buildId: String
}

struct DaemonLocalDatabaseStatus: Codable, Equatable, Sendable {
    let path: String
    let ready: Bool
    let schemaVersion: Int
}

struct DaemonUnavailableProject: Codable, Equatable, Identifiable, Sendable {
    var id: String { projectId }
    let projectId: String
    let bindings: [DaemonProjectBinding]
    let draftCount: Int

    var name: String {
        bindings.first.map { URL(fileURLWithPath: $0.workspaceRoot).lastPathComponent } ?? projectId
    }
}

struct DaemonSyncStatus: Codable, Equatable, Sendable {
    var unavailableProjects: [DaemonUnavailableProject] = []
    let draftSync: DaemonSyncChannelStatus
    let commitSync: DaemonSyncChannelStatus
    let pendingOperationCount: Int
    let failedOperationCount: Int
    let behindDraftCount: Int
    let reconciliationConflictCount: Int
    let lastSuccessAt: String?
}

struct DaemonSyncChannelStatus: Codable, Equatable, Sendable {
    let state: String
    let serverCursor: String?
    let lastAttemptAt: String?
    let lastSuccessAt: String?
    let lastError: APIErrorPayload?
}

struct DaemonSyncRetryRequest: Codable, Sendable {
    let channel: String
}

struct DaemonProjectSyncStatusRequest: Codable, Sendable {
    let projectId: String
}

struct DaemonProjectSyncRetryRequest: Codable, Sendable {
    let projectId: String
    let channel: String
}

struct DaemonRetryResponse: Codable, Sendable {
    let retryId: String
    let started: Bool
}

struct DaemonServerRequest: Codable, Sendable {
    let method: String
    let path: String
    let headers: [String: String]
    let body: String?
}

struct DaemonServerResponse: Codable, Sendable {
    let status: Int
    let headers: [String: String]
    let body: String
}

struct DaemonDraftListQuery: Codable, Sendable {
    let resource: String?
    let status: String?
    let cursor: String?
    let limit: Int?

    init(resource: String? = nil, status: String? = nil, cursor: String? = nil, limit: Int? = nil) {
        self.resource = resource
        self.status = status
        self.cursor = cursor
        self.limit = limit
    }
}

struct DaemonDraftListResponse: Codable, Sendable {
    let items: [DaemonDraftSummary]
    let nextCursor: String?

    init(items: [DaemonDraftSummary], nextCursor: String? = nil) {
        self.items = items
        self.nextCursor = nextCursor
    }
}

struct DaemonDraftDetailRequest: Codable, Sendable {
    let draftId: String
}

struct DaemonDraftDetail: Codable, Sendable {
    let draft: DaemonDraftSummary
    let operations: [DaemonLocalDraftOperation]
}

struct DaemonDraftSummary: Codable, Identifiable, Hashable, Sendable {
    var id: String { draftId }

    let draftId: String
    let projectId: String
    let serverDraftId: String?
    let serverVersion: Int
    let baseCommitId: String?
    let currentCommitId: String?
    let freshness: DraftFreshness
    let hasUpstreamResourceChanges: Bool
    let reconciliation: DraftReconciliationStatus
    let reconciliationCandidateId: String?
    let scope: DaemonDraftScope
    let resourceKind: DaemonResourceKind
    let targetId: String?
    let path: String?
    let status: DaemonLocalDraftStatus
    let createdAt: String
    let updatedAt: String
    let pendingOperationCount: Int
    let failedOperationCount: Int
}

enum DaemonDraftScope: String, Codable, Hashable, Sendable {
    case org
    case project
}

enum DaemonResourceKind: String, Codable, Hashable, Sendable {
    case memory

    /// The daemon now models a single Memory kind, but archived local
    /// databases and older clients still write the legacy context/rule/
    /// workflow values; decode them all to `.memory`.
    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        switch raw {
        case "context", "rule", "workflow", "memory":
            self = .memory
        default:
            throw DecodingError.dataCorrupted(.init(
                codingPath: decoder.codingPath,
                debugDescription: "Unknown daemon resource kind: \(raw)"
            ))
        }
    }
}

enum DaemonLocalDraftStatus: String, Codable, Hashable, Sendable {
    case open
    case submitted
    case merged
    case discarded
}

enum DraftFreshness: String, Codable, Hashable, Sendable {
    case current
    case behind
}

enum DraftReconciliationStatus: String, Codable, Hashable, Sendable {
    case unknown
    case clean
    case conflicts
}

enum DaemonDraftOperationSource: String, Codable, Hashable, Sendable {
    case desktop
    case cli
    case mcpStore = "mcp_store"
    case server
}

enum DaemonDraftSyncState: String, Codable, Hashable, Sendable {
    case queued
    case syncing
    case retrying
    case synced
    case failed
}

struct DaemonLocalDraftOperation: Codable, Identifiable, Sendable {
    var id: String { localOperationId }

    let localOperationId: String
    let resourceKind: DaemonResourceKind
    let operation: DaemonDraftOperation
    let source: DaemonDraftOperationSource
    let syncStatus: DaemonDraftSyncState
    let lastError: String?
    let createdAt: String
    let updatedAt: String
}

struct DaemonDraftOperationRequest: Codable, Sendable {
    let draftId: String?
    let baseCommitId: String?
    let projectId: String
    let scope: DaemonDraftScope
    let resource: DaemonResourceKind
    let op: DaemonDraftOperation
    let source: DaemonDraftOperationSource
}

struct DaemonDraftOperationResponse: Codable, Sendable {
    let localOperationId: String
    let draftId: String
    let queued: Bool
    let syncStatus: DaemonDraftSyncState
}

struct DaemonCreateMemoryDraftsRequest: Encodable, Sendable {
    let projectId: String
    let baseCommitId: String?
    let operations: [DaemonDraftOperation]
}

struct DaemonDraftContent: Codable, Hashable, Sendable {
    let description: String?
    let content: String

    var primaryText: String { content }
    var renderedText: String { content }

    func replacingPrimaryText(with text: String) -> DaemonDraftContent {
        .init(description: description, content: text)
    }
}

enum DaemonDraftOperation: Codable, Sendable {
    case create(path: String, content: DaemonDraftContent, description: String?)
    case update(id: String, content: DaemonDraftContent, description: String?)
    case rename(id: String, newPath: String, description: String?)
    case delete(id: String, description: String?)
    case discard(id: String)

    private enum CodingKeys: String, CodingKey {
        case create
        case update
        case rename
        case delete
        case discard
    }

    private struct CreatePayload: Codable {
        let path: String
        let content: DaemonDraftContent
        let description: String?
    }

    private struct UpdatePayload: Codable {
        let id: String
        let content: DaemonDraftContent
        let description: String?
    }

    private struct RenamePayload: Codable {
        let id: String
        let newPath: String
        let description: String?
    }

    private struct DeletePayload: Codable {
        let id: String
        let description: String?
    }

    private struct DiscardPayload: Codable {
        let id: String
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        if let value = try container.decodeIfPresent(CreatePayload.self, forKey: .create) {
            self = .create(path: value.path, content: value.content, description: value.description)
        } else if let value = try container.decodeIfPresent(UpdatePayload.self, forKey: .update) {
            self = .update(id: value.id, content: value.content, description: value.description)
        } else if let value = try container.decodeIfPresent(RenamePayload.self, forKey: .rename) {
            self = .rename(id: value.id, newPath: value.newPath, description: value.description)
        } else if let value = try container.decodeIfPresent(DeletePayload.self, forKey: .delete) {
            self = .delete(id: value.id, description: value.description)
        } else if let value = try container.decodeIfPresent(DiscardPayload.self, forKey: .discard) {
            self = .discard(id: value.id)
        } else {
            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath, debugDescription: "Unknown daemon draft operation")
            )
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .create(let path, let content, let description):
            try container.encode(CreatePayload(path: path, content: content, description: description), forKey: .create)
        case .update(let id, let content, let description):
            try container.encode(UpdatePayload(id: id, content: content, description: description), forKey: .update)
        case .rename(let id, let newPath, let description):
            try container.encode(RenamePayload(id: id, newPath: newPath, description: description), forKey: .rename)
        case .delete(let id, let description):
            try container.encode(DeletePayload(id: id, description: description), forKey: .delete)
        case .discard(let id):
            try container.encode(DiscardPayload(id: id), forKey: .discard)
        }
    }
}

enum AgentHost: String, Codable, Hashable, Sendable {
    case codex
    case claudeCode = "claude-code"
    case manual
    case zed
    case opencode
    case dsh
    case antigravity
    case unknown

    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = AgentHost(rawValue: raw) ?? .unknown
    }
}

enum RetrievalRunStatus: String, Codable, Hashable, Sendable {
    case running
    case succeeded
    case failed

    var title: String {
        switch self {
        case .running: String(localized: "Running")
        case .succeeded: String(localized: "Succeeded")
        case .failed: String(localized: "Failed")
        }
    }
}

enum RetrievalExclusionReason: String, Codable, CaseIterable, Hashable, Sendable {
    case selected
    case belowRelevance = "below_relevance"
    case overlap
    case perResourceLimit = "per_resource_limit"
    case tokenBudget = "token_budget"
    case fragmentLimit = "fragment_limit"
    case notReranked = "not_reranked"
}

enum RetrievalDeltaAction: String, Codable, CaseIterable, Hashable, Sendable {
    case add
    case replace
    case reuse

    var title: String {
        switch self {
        case .add: String(localized: "Add")
        case .replace: String(localized: "Replace")
        case .reuse: String(localized: "Reuse")
        }
    }
}

struct RetrievalRunListRequest: Codable, Sendable {
    let projectId: String?
    let status: RetrievalRunStatus?
    let cursor: String?
    let limit: Int?
}

struct RetrievalRunRequest: Codable, Sendable {
    let runId: String
}

struct RetrievalRunListResponse: Codable, Sendable {
    let items: [RetrievalRun]
    let nextCursor: String?
}

struct RetrievalStageLatencies: Codable, Equatable, Sendable {
    let effectiveMemoryUs: UInt64
    let indexEnsureUs: UInt64
    let bm25Us: UInt64
    let embeddingUs: UInt64
    let vectorUs: UInt64
    let rrfUs: UInt64
    let rerankUs: UInt64
    let assemblyUs: UInt64
    let persistenceUs: UInt64
    let totalUs: UInt64
}

struct RetrievalRun: Codable, Identifiable, Equatable, Sendable {
    var id: String { runId }

    let runId: String
    let projectId: String
    let query: String
    let activationStateFingerprint: String
    let status: RetrievalRunStatus
    let effectiveHash: String?
    let indexRevision: String?
    let resourceCount: UInt64
    let unitCount: UInt64
    let parserVersion: String?
    let chunkerVersion: String?
    let modelRevision: String?
    let rankingProfile: String?
    let latencies: RetrievalStageLatencies
    let returnedFragmentCount: UInt64
    let returnedTokenCount: UInt64
    let errorStage: String?
    let errorCode: String?
    let errorSummary: String?
    let createdAt: String
    let completedAt: String?
    let evaluationCaseId: String?
    let evaluationCaseStatus: EvaluationCaseStatus?
}

struct RetrievalSourceLocator: Codable, Equatable, Sendable {
    let type: String
    let startByte: UInt64
    let endByte: UInt64
    let headingPath: [String]
}

struct RetrievalCandidate: Codable, Identifiable, Equatable, Sendable {
    var id: String { unitKey }

    let unitKey: String
    let resourceId: String
    let scope: DaemonDraftScope
    let kind: DaemonResourceKind
    let path: String
    let headingPath: [String]
    let locator: RetrievalSourceLocator
    let contentHash: String
    let resourceContentHash: String
    let tokenCount: UInt64
    let evidenceExcerpt: String
    let exactRank: UInt64?
    let bm25Rank: UInt64?
    let bm25Score: Double?
    let vectorRank: UInt64?
    let vectorScore: Double?
    let rrfRank: UInt64?
    let rrfScore: Double?
    let rerankerRank: UInt64?
    let rerankerLogit: Double?
    let rerankerRelevance: Double?
    let finalRank: UInt64?
    let selected: Bool
    let exclusionReason: RetrievalExclusionReason
    let deltaAction: RetrievalDeltaAction?
}

struct RetrievalRunDetail: Codable, Sendable {
    let run: RetrievalRun
    let candidates: [RetrievalCandidate]
    let evaluationCase: EvaluationCase?
    let evidence: [EvaluationEvidence]
    let evidenceSuggestions: [EvaluationEvidenceSuggestion]
    let report: RetrievalBenchmarkReport?
}

struct CreateEvaluationCaseRequest: Codable, Sendable {
    let runId: String
}

struct ResolveEvaluationCaseRequest: Codable, Sendable {
    let caseId: String
    let expectedVersion: UInt64
    let evidence: [EvaluationEvidenceInput]
    let noneMatched: Bool
}

struct EvaluationEvidenceInput: Codable, Hashable, Sendable {
    let resourceId: String
    let unitKey: String?
}

enum EvaluationCaseStatus: String, Codable, Equatable, Sendable {
    case draft
    case needsEvidence = "needs_evidence"
    case ready
}

struct EvaluationCase: Codable, Identifiable, Equatable, Sendable {
    var id: String { caseId }

    let caseId: String
    let sourceRunId: String
    let corpusId: String
    let projectId: String
    let query: String
    let status: EvaluationCaseStatus
    let version: UInt64
    let createdAt: String
    let updatedAt: String
}

struct EvaluationEvidence: Codable, Identifiable, Equatable, Sendable {
    var id: String { evidenceId }

    let evidenceId: String
    let caseId: String
    let resourceId: String
    let unitKey: String?
    let evidenceExcerpt: String
}

enum RetrievalFailureStage: String, Codable, Equatable, Sendable {
    case fusion
    case reranking
    case assembly
}

struct EvaluationEvidenceSuggestion: Codable, Identifiable, Equatable, Sendable {
    var id: String { unitKey }

    let resourceId: String
    let unitKey: String
    let path: String
    let headingPath: [String]
    let evidenceExcerpt: String
    let modelRelevance: Double?
    let likelyFailureStage: RetrievalFailureStage
    let exclusionReason: RetrievalExclusionReason
}

struct RetrievalBenchmarkMetrics: Codable, Equatable, Sendable {
    let caseCount: UInt64
    let recallAt20: Double
    let ndcgAt10: Double
    let mrr: Double
    let resourceDiversity: Double
    let scopeViolation: Double
    let staleResult: Double
    let warmP50Us: UInt64
    let warmP95Us: UInt64
}

struct RetrievalBenchmarkReport: Codable, Equatable, Sendable {
    let variants: [String: RetrievalBenchmarkMetrics]
}

struct EvaluationCaseDetail: Codable, Sendable {
    let evaluationCase: EvaluationCase
    let evidence: [EvaluationEvidence]
    let evidenceSuggestions: [EvaluationEvidenceSuggestion]
    let report: RetrievalBenchmarkReport?
}

struct ClearRetrievalRunsRequest: Codable, Sendable {
    let projectId: String?
}

struct ClearRetrievalRunsResponse: Codable, Sendable {
    let deletedRunCount: UInt64
}

struct ExportEvaluationSetRequest: Codable, Sendable {
    let projectId: String?
    let caseIds: [String]
}

struct ExportEvaluationSetResponse: Codable, Sendable {
    let fixtureJson: String
    let report: RetrievalBenchmarkReport
}

struct ListRecallsRequest: Codable, Sendable {
    let workspaceRoot: String?
    let projectId: String?
    let limit: Int?
    let cursor: String?

    init(workspaceRoot: String? = nil, projectId: String? = nil, limit: Int? = nil, cursor: String? = nil) {
        self.workspaceRoot = workspaceRoot
        self.projectId = projectId
        self.limit = limit
        self.cursor = cursor
    }
}

struct ListRecallsResponse: Codable, Sendable {
    let sessions: [RecallSessionSummary]
    let workspaceRoots: [String]
    var nextCursor: String? = nil
}

struct RecallSessionSummary: Codable, Identifiable, Sendable {
    var id: String { "\(host.rawValue):\(sessionId)" }
    let host: AgentHost
    let sessionId: String
    let title: String?
    let workspaceRoot: String
    let createdAt: Int64?
    let sessionToken: String

    var activityDisplayTitle: String {
        title.flatMap { $0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : $0 }
            ?? String(localized: "Agent activity")
    }
}

struct GetRecallSessionRequest: Codable, Sendable {
    let sessionToken: String
    var offset: Int? = nil
    var limit: Int? = nil
}

struct GetRecallSessionResponse: Codable, Sendable {
    let session: RecallSession
    let totalTasks: Int
    let nextOffset: Int?
}

struct GetRecallFragmentRequest: Codable, Sendable {
    let workspaceRoot: String
    let runId: String
    let unitKey: String
}

struct GetRecallFragmentResponse: Codable, Sendable {
    let fragment: RecallFragment
}

struct RecallSession: Codable, Identifiable, Sendable {
    var id: String { "\(host.rawValue):\(sessionId)" }

    let host: AgentHost
    let sessionId: String
    let title: String?
    let workspaceRoot: String
    let createdAt: Int64?
    var tasks: [RecallTask]
}

struct RecallTask: Codable, Identifiable, Sendable {
    var id: String { messageId }

    let messageId: String
    let text: String
    let time: Int64?
    let activations: [RecallActivation]
}

struct RecallActivation: Codable, Identifiable, Sendable {
    var id: String { callId }

    let toolName: String
    let callId: String
    let query: String
    let state: String?
    let time: Int64?
    let runId: String?
    let runStatus: String?
    var totalUs: UInt64? = nil
    let fragments: [RecallFragment]
    let resultError: String?
}

struct RecallFragment: Codable, Identifiable, Sendable {
    var id: String { unitKey }

    let action: String?
    let unitKey: String
    let resourceId: String
    let scope: DaemonDraftScope?
    let path: String
    let headingPath: [String]
    let content: String
    let finalRank: UInt64?
    let truncated: Bool
}

enum DaemonXPCError: LocalizedError, Sendable {
    case invalidRequest
    case connectionFailed(detail: String? = nil)
    case requestTimedOut(timeout: TimeInterval? = nil)
    case invalidReply
    case daemon(APIErrorPayload)

    var errorDescription: String? { ClientFailure(self).message }
}
