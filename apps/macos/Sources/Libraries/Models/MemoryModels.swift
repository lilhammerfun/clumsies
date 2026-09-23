import Foundation

enum WorkspaceSection: String, CaseIterable, Identifiable, Sendable {
    case dashboard
    case inbox
    case memory
    case bundles
    case reviews
    case sessions

    var id: String { rawValue }

    var title: String {
        switch self {
        case .dashboard: String(localized: "Dashboard")
        case .inbox: String(localized: "Inbox")
        case .memory: String(localized: "Memory")
        case .bundles: String(localized: "Bundles")
        case .reviews: String(localized: "Reviews")
        case .sessions: String(localized: "Activity")
        }
    }

    var symbol: String {
        switch self {
        case .dashboard: "chart.bar.xaxis"
        case .inbox: "tray"
        case .memory: "brain"
        case .bundles: "shippingbox"
        case .reviews: "checkmark.bubble"
        case .sessions: "bubble.left.and.bubble.right"
        }
    }
}

enum MemoryKind: String, CaseIterable, Codable, Identifiable, Sendable {
    case context
    case rules
    case workflows

    var id: String { rawValue }

    var title: String {
        switch self {
        case .context: String(localized: "Context")
        case .rules: String(localized: "Rules")
        case .workflows: String(localized: "Workflow")
        }
    }

    var singularTitle: String {
        switch self {
        case .context: String(localized: "Context")
        case .rules: String(localized: "Rule")
        case .workflows: String(localized: "Workflow")
        }
    }

    var symbol: String {
        switch self {
        case .context: "doc.text"
        case .rules: "checklist"
        case .workflows: "point.3.connected.trianglepath.dotted"
        }
    }

    /// The daemon and Server now model a single Memory kind; the legacy
    /// context/rules/workflows cases remain as UI-level creation defaults and
    /// path conventions only.
    var daemonKind: DaemonResourceKind { .memory }

    func supportsMarkdownPreview(path: String) -> Bool {
        switch self {
        case .rules, .workflows:
            return true
        case .context:
            let pathExtension = URL(fileURLWithPath: path).pathExtension.lowercased()
            return pathExtension == "md" || pathExtension == "markdown"
        }
    }

    /// Resources are no longer kind-tagged on the wire; map the unified
    /// daemon kind to the default UI creation kind.
    init(_ daemonKind: DaemonResourceKind) {
        self = .context
    }
}

enum MemoryScope: String, Codable, Sendable {
    case org
    case project
}

struct EditableMemoryDocument: Hashable, Sendable {
    var title: String
    var path: String
    var body: String
}

struct MemoryResource: Identifiable, Hashable, Sendable {
    let id: String
    let scope: MemoryScope
    let projectId: String?
    let projectName: String?
    let kind: MemoryKind
    let contentHash: String
    let updatedAt: String
    let refCommitId: String?
    var contentLoaded: Bool
    var document: EditableMemoryDocument
    var orgSource: OrgMemorySource? = nil
}

struct LocalDraft: Identifiable, Hashable, Sendable {
    let id: String
    let projectId: String
    let serverId: String?
    let serverVersion: Int
    let baseCommitId: String?
    let currentCommitId: String?
    let freshness: DraftFreshness
    let hasUpstreamResourceChanges: Bool
    let reconciliation: DraftReconciliationStatus
    let reconciliationCandidateId: String?
    let scope: MemoryScope
    let kind: MemoryKind
    let targetId: String?
    let status: DaemonLocalDraftStatus
    let origin: DaemonDraftOperationSource
    let syncStatus: DaemonDraftSyncState
    let updatedAt: String
    var document: EditableMemoryDocument
    var isDeletion: Bool
    var documentBaselineAvailable: Bool = true
    var orgSource: OrgMemorySource? = nil
}

struct MemoryListItem: Identifiable, Hashable, Sendable {
    let id: String
    let resource: MemoryResource?
    let draft: LocalDraft?
    let inherited: Bool
    /// Project whose effective-memory view produced this item. This is the
    /// stable carrier for a new local Draft on selected Org authority; it is
    /// deliberately distinct from resource ownership.
    var projectContextId: String? = nil

    var document: EditableMemoryDocument {
        draft?.document ?? resource?.document ?? .init(
            title: String(localized: "Untitled"),
            path: "",
            body: ""
        )
    }

    var kind: MemoryKind { draft?.kind ?? resource?.kind ?? .context }
    var scope: MemoryScope { draft?.scope ?? resource?.scope ?? .project }
    var projectId: String? { draft?.projectId ?? resource?.projectId ?? projectContextId }
    var contentLoaded: Bool {
        draft?.documentBaselineAvailable ?? (resource?.contentLoaded == true)
    }

    var supportsMarkdownPreview: Bool {
        draft?.isDeletion != true && kind.supportsMarkdownPreview(path: document.path)
    }
}

/// Identifies one editor/sync session in the Project view that owns it.
/// Org authority and each Project overlay can address the same resource id,
/// so the bare resource id is not a safe key for mutable document state.
struct MemoryDocumentSessionKey: Hashable, Sendable {
    let projectId: String
    let itemId: String
}

struct PersonalBundle: Identifiable, Hashable, Sendable {
    let id: String
    var name: String
    var description: String
    var resourceIds: [String]
    let revision: Int
    let updatedAt: String
}

struct ReviewRecord: Identifiable, Hashable, Sendable {
    let id: String
    let projectId: String
    let draftId: String
    let title: String
    let description: String
    let author: UserReference
    let status: String
    let version: Int
    let decisionBody: String?
    let approvedResultHash: String?
    let decidedBy: UserReference?
    let decidedAt: String?
    let freshness: DraftFreshness
    let reconciliation: DraftReconciliationStatus
    let reconciliationCandidateId: String?
    let currentCommitId: String?
    let updatedAt: String
    var draftIds: [String] = []
    var autoRebased = false
    var scope: MemoryScope = .org
    var orgContribution: OrgContribution? = nil
    var projectSource: ProjectReviewSource? = nil
}

struct ReviewChangeSources: Sendable {
    let baseContent: String?
    let currentContent: String?
    let draftContent: String?
    let resolutionContent: String?
    let proposedPath: String?
    let operationLabels: [String]
}

struct ProjectState: Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let refCommitId: String?
    let refEtag: String
    let selectedOrgResourceIds: Set<String>
    let orgSelectionRevision: Int
    let isLoaded: Bool
}

struct RuntimeState: Equatable, Sendable {
    let health: DaemonHealth
    let sync: DaemonSyncStatus?
    let serverDataSource: String
}

enum WorkbenchTabMode: String, CaseIterable, Hashable, Sendable {
    case preview
    case source
    case diff

    var title: String {
        switch self {
        case .preview: String(localized: "Preview")
        case .source: String(localized: "Source")
        case .diff: String(localized: "Diff")
        }
    }

    var symbol: String {
        switch self {
        case .preview: "eye"
        case .source: "doc.plaintext"
        case .diff: "arrow.left.arrow.right"
        }
    }
}

struct WorkbenchTab: Identifiable, Hashable, Sendable {
    var id: String {
        "\(section.rawValue):\(projectId ?? "shared"):\(itemId)"
    }

    let section: WorkspaceSection
    let projectId: String?
    let itemId: String
    var mode: WorkbenchTabMode
    var title: String

    func isVisible(in section: WorkspaceSection, projectId: String?) -> Bool {
        guard self.section == section else { return false }
        guard section == .memory else { return true }
        // A document tab belongs to the view context in which it was opened.
        // This keeps an Org authority tab separate from a Project-local draft
        // overlay of the same memory.
        return self.projectId == projectId
    }
}

enum ReviewRequestError: UserFacingError, Sendable {
    case draftNotSynchronized
    case mixedScopes
    case reconciliationRequired
    case mixedProjects
    case reviewChanged

    var errorDescription: String? {
        switch self {
        case .draftNotSynchronized:
            String(localized: "Wait for this draft to finish syncing before requesting a review.")
        case .mixedScopes:
            String(localized: "All drafts in a Review must publish to the same Project or Organization.")
        case .reconciliationRequired:
            String(localized: "Merge the latest remote version before requesting a review.")
        case .mixedProjects:
            String(localized: "All drafts in a review must belong to the same Project.")
        case .reviewChanged:
            String(localized: "This Review changed on the Server. Review its latest state before deciding.")
        }
    }
}

enum DocumentSyncError: UserFacingError, Equatable, Sendable {
    case checkoutNoLongerCurrent
    case draftUploadFailed(String?)
    case draftUploadTimedOut
    case mutationWhileSynchronizing

    var errorDescription: String? {
        switch self {
        case .checkoutNoLongerCurrent:
            String(localized: "The remote version changed again. Refresh sync status and try again.")
        case .draftUploadFailed(let message):
            message ?? String(localized: "The local draft could not be uploaded. Retry sync before reviewing remote changes.")
        case .draftUploadTimedOut:
            String(localized: "The local draft is still uploading. Wait a moment and try Sync again.")
        case .mutationWhileSynchronizing:
            String(localized: "Remote changes are being checked for this document. Wait for Sync to finish before editing it.")
        }
    }
}

enum ProjectSetupError: UserFacingError, Sendable {
    case bundledAgentRuntimeMissing
    case codexHostMissing
    case bundleNotFound
    case bundleContainsUnavailableMemory

    var errorDescription: String? {
        switch self {
        case .bundledAgentRuntimeMissing:
            String(localized: "The clumsiesd Agent runtime is missing from this app build.")
        case .codexHostMissing:
            String(localized: "Install or update the Codex app before repairing its Clumsies Plugin.")
        case .bundleNotFound:
            String(localized: "The selected Bundle is no longer available.")
        case .bundleContainsUnavailableMemory:
            String(localized: "The selected Bundle contains memory that is not available in the Organization.")
        }
    }
}

enum ProjectMemorySelectionError: UserFacingError, Sendable {
    case activeDrafts
    case invalidOrgResources
    case projectUnavailable

    var errorDescription: String? {
        switch self {
        case .activeDrafts:
            String(localized: "Discard or finish this Project's LocalDraft before removing its Organization Memory.")
        case .invalidOrgResources:
            String(localized: "Only current Organization memory can be added to or removed from a Project.")
        case .projectUnavailable:
            String(localized: "The selected Project is no longer available.")
        }
    }
}
