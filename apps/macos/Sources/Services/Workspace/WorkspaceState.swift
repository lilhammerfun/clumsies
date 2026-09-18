import Foundation

enum DocumentSessionCommand: Equatable, Sendable {
    case requestReview(sessionKey: MemoryDocumentSessionKey, draft: LocalDraft)
    case discardDraft(sessionKey: MemoryDocumentSessionKey, draft: LocalDraft)
    case applyReconciliation(sessionKey: MemoryDocumentSessionKey)
    case closeReconciliation(sessionKey: MemoryDocumentSessionKey)
    case moveToTrash(sessionKey: MemoryDocumentSessionKey)

    var sessionKey: MemoryDocumentSessionKey {
        switch self {
        case .requestReview(let sessionKey, _),
             .discardDraft(let sessionKey, _),
             .applyReconciliation(let sessionKey),
             .closeReconciliation(let sessionKey),
             .moveToTrash(let sessionKey):
            sessionKey
        }
    }
}

struct DocumentReconciliationToolbarState: Equatable, Sendable {
    let sessionKey: MemoryDocumentSessionKey
    let isLoading: Bool
    let canUpdate: Bool
    let isUpdating: Bool
}

enum ApplicationPhase: Equatable, Sendable {
    case launching
    case authenticationRequired
    case loading
    case ready
    case failed(String)
}

enum WorkspaceCollectionLoadState: Equatable, Sendable {
    case loading
    case loaded
    case failed(String)

    var isLoading: Bool {
        self == .loading
    }

    var failureMessage: String? {
        guard case .failed(let message) = self else { return nil }
        return message
    }
}

enum SyncRetryOutcome: Equatable, Sendable {
    case completed
    case failed(String)
    case cancelled
}

struct SyncRetryKey: Hashable, Sendable {
    let channel: String
    let projectId: String?
}

struct WorkspaceSnapshot: Sendable {
    let account: UserReference
    let organization: OrganizationReference
    let capabilities: Set<String>
    let projects: [ProjectState]
    var projectRoles: [String: ProjectMemberRole] = [:]
    let activeProjectId: String?
    let orgRefCommitId: String?
    let orgRefEtag: String
    let resources: [MemoryResource]
    let runtime: RuntimeState
    let legacyAgentAdapterConflicts: [DaemonLegacyAgentAdapterConflict]
    let legacyAgentAdapterInspectionWarning: String?
}

struct LocalAgentAdapterReconciliationResult: Equatable, Sendable {
    let conflicts: [DaemonLegacyAgentAdapterConflict]
    let inspectionWarning: String?
}

enum WorkspaceLoadError: LocalizedError, Sendable {
    case authenticationRequired
    case noProjects
    case sharedStateChangedDuringLoad

    var errorDescription: String? {
        switch self {
        case .authenticationRequired: "Sign in to connect Clumsies to your organization."
        case .noProjects: "The signed-in account has no accessible project."
        case .sharedStateChangedDuringLoad:
            "Shared memory changed while the workspace was loading. Refresh to load one consistent version."
        }
    }
}

enum AdministrationError: LocalizedError, Sendable {
    case forbidden
    case unavailable
    case stale
    case busy

    var errorDescription: String? {
        switch self {
        case .forbidden:
            "Organization administrator access is required."
        case .unavailable:
            "Load this organization page before making changes."
        case .stale:
            "This organization page is showing cached data. Refresh with a live Server connection before making changes."
        case .busy:
            "Another organization operation is still in progress."
        }
    }
}

enum MemoryValidationError: LocalizedError, Sendable {
    case invalidPath(String)
    case emptyRule
    case memoryCannotBeRenamed

    var errorDescription: String? {
        switch self {
        case .invalidPath(let message): message
        case .emptyRule: "A Rule needs content."
        case .memoryCannotBeRenamed:
            "This memory is no longer available to rename."
        }
    }
}

enum ReviewRequestError: LocalizedError, Sendable {
    case draftNotSynchronized
    case legacyProjectDraftCannotBePublished
    case reconciliationRequired
    case mixedProjects
    case reviewChanged

    var errorDescription: String? {
        switch self {
        case .draftNotSynchronized:
            "Wait for this draft to finish syncing before requesting a review."
        case .legacyProjectDraftCannotBePublished:
            "Legacy Project-scoped drafts are read-only and cannot be published."
        case .reconciliationRequired:
            "Merge the latest shared version before requesting a review."
        case .mixedProjects:
            "All drafts in a review must belong to the same Project."
        case .reviewChanged:
            "This Review changed on the Server. Review its latest state before deciding."
        }
    }
}

enum DocumentSyncError: LocalizedError, Equatable, Sendable {
    case checkoutNoLongerCurrent
    case draftUploadFailed(String?)
    case draftUploadTimedOut
    case mutationWhileSynchronizing

    var errorDescription: String? {
        switch self {
        case .checkoutNoLongerCurrent:
            "The shared version changed again. Refresh sync status and try again."
        case .draftUploadFailed(let message):
            message ?? "The local draft could not be uploaded. Retry sync before reviewing shared changes."
        case .draftUploadTimedOut:
            "The local draft is still uploading. Wait a moment and try Sync again."
        case .mutationWhileSynchronizing:
            "Shared changes are being prepared for this document. Wait for Sync to finish before editing it."
        }
    }
}

enum DocumentDiffError: LocalizedError, Equatable, Sendable {
    case baselineUnavailable

    var errorDescription: String? {
        switch self {
        case .baselineUnavailable:
            "The previous shared content is unavailable, so an accurate Diff cannot be shown."
        }
    }
}

enum DraftUploadBarrierDecision: Equatable, Sendable {
    case wait
    case ready
    case failed(String?)
}

struct StaleResourceSyncSnapshot: Equatable, Sendable {
    let projectId: String
    let observedProjectRefCommitId: String?
    let observedSelectedOrgResourceIds: Set<String>
    let observedOrgSelectionRevision: Int
    let authoritativeCommitId: String
    let authoritativeRefEtag: String?
    let selectedOrgResourceIds: Set<String>
    let orgSelectionRevision: Int
    let generation: UUID
    let local: MemoryResource?
    let remote: MemoryResource?
}

enum DocumentPathChangeSource: String, Equatable, Sendable {
    case draft
    case shared
    case draftAndShared
}

struct DocumentPathChange: Equatable, Sendable {
    let source: DocumentPathChangeSource
    let from: String?
    let to: String?
}

struct DocumentDiffResult: Equatable, Sendable {
    let presentation: UnifiedDiffPresentation?
    let pathChanges: [DocumentPathChange]
}

struct DocumentRenamePlan: Equatable, Sendable {
    let targetId: String
    let newPath: String
}

enum ReviewMenuAction: Sendable, Equatable {
    case approve
    case reject
    case merge
    case resubmit

    func isAvailable(
        for review: ReviewRecord,
        canDecideReviews: Bool,
        canMergeReviews: Bool,
        isAuthor: Bool
    ) -> Bool {
        switch self {
        case .approve:
            return review.status == "open" && canDecideReviews && canMergeReviews
        case .reject:
            return review.status == "open" && canDecideReviews
        case .merge:
            return review.status == "approved"
                && review.approvedResultHash?.isEmpty == false
                && canMergeReviews
        case .resubmit:
            return review.status == "rejected" && isAuthor
        }
    }
}

struct ReviewDecisionReadiness: Equatable, Sendable {
    let reviewId: String
    let reviewVersion: Int
    let status: String
    let approvedResultHash: String?
    let freshness: DraftFreshness
    let reconciliation: DraftReconciliationStatus
    let currentCommitId: String?

    init(review: ReviewRecord) {
        reviewId = review.id
        reviewVersion = review.version
        status = review.status
        approvedResultHash = review.approvedResultHash
        freshness = review.freshness
        reconciliation = review.reconciliation
        currentCommitId = review.currentCommitId
    }

    func matches(_ review: ReviewRecord) -> Bool {
        self == ReviewDecisionReadiness(review: review)
    }
}

enum ProjectSetupError: LocalizedError, Sendable {
    case bundledAgentRuntimeMissing
    case codexHostMissing
    case bundleNotFound
    case bundleContainsUnavailableMemory

    var errorDescription: String? {
        switch self {
        case .bundledAgentRuntimeMissing:
            "The clumsiesd Agent runtime is missing from this app build."
        case .codexHostMissing:
            "Install or update the Codex app before repairing its Clumsies Plugin."
        case .bundleNotFound:
            "The selected Bundle is no longer available."
        case .bundleContainsUnavailableMemory:
            "The selected Bundle contains memory that is not available in the Organization."
        }
    }
}

enum ProjectMemorySelectionError: LocalizedError, Sendable {
    case activeDrafts
    case invalidOrgResources
    case projectUnavailable

    var errorDescription: String? {
        switch self {
        case .activeDrafts:
            "Discard or finish this Project's LocalDraft before removing its Organization Memory."
        case .invalidOrgResources:
            "Only current Organization memory can be added to or removed from a Project."
        case .projectUnavailable:
            "The selected Project is no longer available."
        }
    }
}

enum ProjectOrgSelectionMutation: Sendable {
    case add
    case remove

    func applying(_ resourceIds: Set<String>, to current: Set<String>) -> Set<String> {
        switch self {
        case .add:
            current.union(resourceIds)
        case .remove:
            current.subtracting(resourceIds)
        }
    }
}

struct DraftInventoryPlan: Equatable, Sendable {
    let refreshIds: Set<String>
    let terminalIds: Set<String>
}

enum WorkspaceRefreshCadence {
    static let syncStatus: Duration = .seconds(2)
    static let synchronizedData: Duration = .seconds(30)
}
