import Foundation

enum WorkspaceSection: String, CaseIterable, Identifiable, Sendable {
    case memory
    case bundles
    case reviews
    case sessions

    var id: String { rawValue }

    var title: String {
        switch self {
        case .memory: "Memory"
        case .bundles: "Bundles"
        case .reviews: "Reviews"
        case .sessions: "Activity"
        }
    }

    var symbol: String {
        switch self {
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
        case .context: "Context"
        case .rules: "Rules"
        case .workflows: "Workflow"
        }
    }

    var singularTitle: String {
        switch self {
        case .context: "Context"
        case .rules: "Rule"
        case .workflows: "Workflow"
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
            title: "Untitled",
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
}

struct ReviewChangeSources: Sendable {
    let baseContent: String?
    let currentContent: String?
    let draftContent: String?
    let resolutionContent: String?
    let proposedPath: String?
    let operationLabels: [String]
}

struct ReviewFileChange: Sendable {
    let detail: ReviewDraftDetail
    let sources: ReviewChangeSources
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
        case .preview: "Preview"
        case .source: "Source"
        case .diff: "Diff"
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
