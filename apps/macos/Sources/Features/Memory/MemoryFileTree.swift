import AppKit
import SwiftUI

struct FileTreeDirectoryClickResult {
    let selection: Set<String>
    let anchorId: String?
    let togglesDirectory: Bool
}

enum FileTreeSelectionInteraction {
    static func directoryClick(
        nodeId: String,
        visibleNodeIds: [String],
        currentSelection: Set<String>,
        anchorId: String?,
        modifierFlags: NSEvent.ModifierFlags
    ) -> FileTreeDirectoryClickResult {
        if modifierFlags.contains(.shift) {
            let effectiveAnchor = anchorId ?? nodeId
            guard let anchorIndex = visibleNodeIds.firstIndex(of: effectiveAnchor),
                  let nodeIndex = visibleNodeIds.firstIndex(of: nodeId) else {
                return .init(
                    selection: [nodeId],
                    anchorId: nodeId,
                    togglesDirectory: false
                )
            }
            let range = min(anchorIndex, nodeIndex) ... max(anchorIndex, nodeIndex)
            let rangeSelection = Set(range.map { visibleNodeIds[$0] })
            return .init(
                selection: modifierFlags.contains(.command)
                    ? currentSelection.union(rangeSelection)
                    : rangeSelection,
                anchorId: effectiveAnchor,
                togglesDirectory: false
            )
        }

        if modifierFlags.contains(.command) {
            var selection = currentSelection
            if selection.contains(nodeId) {
                selection.remove(nodeId)
            } else {
                selection.insert(nodeId)
            }
            return .init(
                selection: selection,
                anchorId: nodeId,
                togglesDirectory: false
            )
        }

        guard modifierFlags.intersection([.option, .control]).isEmpty else {
            return .init(
                selection: currentSelection,
                anchorId: anchorId,
                togglesDirectory: false
            )
        }

        return .init(
            selection: [nodeId],
            anchorId: nodeId,
            togglesDirectory: true
        )
    }
}

struct VisibleFileTreeNode: Identifiable {
    let node: FileTreeNode
    let depth: Int

    var id: String { node.id }
}

struct FileTreeNode: Identifiable {
    let id: String
    let name: String
    let item: MemoryListItem?
    let children: [FileTreeNode]?

    static func build(_ items: [MemoryListItem]) -> [FileTreeNode] {
        var itemsById: [String: MemoryListItem] = [:]
        let pathItems = items.map { item in
            itemsById[item.id] = item
            return PathTreeItem(
                id: item.id,
                path: treePath(for: item),
                fallbackName: item.document.title
            )
        }

        func convert(_ node: PathTreeNode) -> FileTreeNode {
            FileTreeNode(
                id: node.id,
                name: node.name,
                item: node.item.flatMap { itemsById[$0.id] },
                children: node.children.map { $0.map(convert) }
            )
        }

        return PathTreeNode.build(pathItems).map(convert)
    }

    static func treePath(for item: MemoryListItem) -> String {
        treePath(for: item.document.path, kind: item.kind)
    }

    static func treePath(for documentPath: String, kind: MemoryKind) -> String {
        var components = documentPath.split(separator: "/").map(String.init)
        if kind == .workflows, components.first == "workflow", components.count > 1 {
            components.removeFirst()
        }
        return components.joined(separator: "/")
    }

    static func documentPath(fromTreePath path: String, for item: MemoryListItem) -> String {
        guard item.kind == .workflows,
              item.document.path.split(separator: "/").first == "workflow" else {
            return path
        }
        return "workflow/\(path)"
    }

    static func directoryPath(from id: String) -> String? {
        let prefix = "directory:"
        guard id.hasPrefix(prefix) else { return nil }
        let path = String(id.dropFirst(prefix.count))
        return path.isEmpty ? nil : path
    }

    static func directoryIds(in nodes: [FileTreeNode]) -> Set<String> {
        nodes.reduce(into: Set<String>()) { result, node in
            guard let children = node.children else { return }
            result.insert(node.id)
            result.formUnion(directoryIds(in: children))
        }
    }

    static func allIds(in nodes: [FileTreeNode]) -> [String] {
        nodes.flatMap { node in
            [node.id] + (node.children.map { allIds(in: $0) } ?? [])
        }
    }

    static func node(withId id: String, in nodes: [FileTreeNode]) -> FileTreeNode? {
        for node in nodes {
            if node.id == id { return node }
            if let children = node.children,
               let match = self.node(withId: id, in: children) {
                return match
            }
        }
        return nil
    }

    static func selectedDirectory(
        in nodes: [FileTreeNode],
        selectedNodeIds: Set<String>
    ) -> FileTreeNode? {
        guard selectedNodeIds.count == 1,
              let id = selectedNodeIds.first,
              let node = node(withId: id, in: nodes),
              node.item == nil else {
            return nil
        }
        return node
    }

    static func items(
        in nodes: [FileTreeNode],
        selectedNodeIds: Set<String>
    ) -> [MemoryListItem] {
        var selectedItems: [String: MemoryListItem] = [:]

        func collect(_ node: FileTreeNode) {
            if let item = node.item {
                selectedItems[item.id] = item
            }
            node.children?.forEach(collect)
        }

        func visit(_ node: FileTreeNode) {
            if selectedNodeIds.contains(node.id) {
                collect(node)
            } else {
                node.children?.forEach(visit)
            }
        }

        nodes.forEach(visit)
        return selectedItems.values.sorted {
            $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending
        }
    }

    static func visibleNodes(
        _ nodes: [FileTreeNode],
        expandedDirectoryIds: Set<String>,
        depth: Int = 0
    ) -> [VisibleFileTreeNode] {
        nodes.flatMap { node in
            var result = [VisibleFileTreeNode(node: node, depth: depth)]
            if expandedDirectoryIds.contains(node.id), let children = node.children {
                result.append(contentsOf: visibleNodes(
                    children,
                    expandedDirectoryIds: expandedDirectoryIds,
                    depth: depth + 1
                ))
            }
            return result
        }
    }
}

struct MemoryDirectoryRenameChange: Hashable, Sendable {
    let item: MemoryListItem
    let newPath: String
}

struct MemoryDirectoryRenamePlan: Hashable, Sendable {
    let changes: [MemoryDirectoryRenameChange]
}

struct MemoryDirectoryDeletionPlan: Hashable, Sendable {
    let itemsToDelete: [MemoryListItem]
    let draftsToDiscard: [LocalDraft]
}

enum MemoryDirectoryMutationError: LocalizedError, Equatable {
    case invalidDirectory
    case invalidName
    case readOnly
    case pathCollision(String)

    var errorDescription: String? {
        switch self {
        case .invalidDirectory:
            String(localized: "This folder no longer contains any memory.")
        case .invalidName:
            String(localized: "Choose a different folder name without a slash.")
        case .readOnly:
            String(localized: "Every memory in the folder must be editable before the folder can be changed.")
        case .pathCollision(let path):
            String(localized: "The folder cannot be renamed because \(path) already exists.")
        }
    }
}

/// Pure classification of file-tree context menu operations (design v2).
///
/// Menu = generic document operations (standard macOS conventions) + domain
/// operations (Memory project membership and drafts). Add to Project exists
/// only in the read-only Org view; Draft proposals and Remove from Project
/// exist only inside an explicit Project context.
enum MemoryFileTreeMenu {
    static func isReviewSelectionReady(_ drafts: [LocalDraft]) -> Bool {
        // The request sheet reconciles behind drafts, including conflicts.
        !drafts.isEmpty && drafts.allSatisfy {
            $0.syncStatus == .synced && $0.serverId != nil
        }
    }

    /// One directory Review contains every open Organization Draft below the
    /// selection. Unchanged files and legacy Project authority are excluded.
    static func reviewableDrafts(
        _ items: [MemoryListItem],
        inOrgView: Bool
    ) -> [LocalDraft] {
        guard !inOrgView else { return [] }
        return items.compactMap(\.draft).filter {
            $0.status == .open && ReviewsModel.canRequestReview($0)
        }
    }

    static func discardableDrafts(
        _ items: [MemoryListItem],
        inOrgView: Bool
    ) -> [LocalDraft] {
        guard !inOrgView else { return [] }
        var seen = Set<String>()
        return items.compactMap(\.draft).filter {
            $0.status != .discarded
                && $0.status != .merged
                && seen.insert($0.id).inserted
        }
            .sorted {
                $0.document.path.localizedStandardCompare($1.document.path)
                    == .orderedAscending
            }
    }

    /// Rename is an organization-authority proposal. Project views only
    /// expose it for Org resources that are still selected by that project.
    /// The Org overview is authority-only/read-only because it has no
    /// unambiguous Project carrier for a LocalDraft.
    /// Pure create Drafts keep their full local document and can be renamed;
    /// target-backed orphan rows and legacy Project authority stay read-only.
    static func canRename(_ item: MemoryListItem, inOrgView: Bool) -> Bool {
        guard !inOrgView, item.draft?.isDeletion != true else {
            return false
        }
        if item.resource?.scope == .org { return item.inherited }
        return item.resource == nil
            && item.draft?.scope == .org
            && item.draft?.targetId == nil
    }

    static func directoryRenamePlan(
        directoryId: String,
        newName: String,
        items: [MemoryListItem],
        occupiedPaths: Set<String>,
        occupiedTreePaths: Set<String>,
        inOrgView: Bool
    ) throws -> MemoryDirectoryRenamePlan {
        let name = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name != ".", name != "..", !name.contains("/") else {
            throw MemoryDirectoryMutationError.invalidName
        }
        guard let sourceDirectory = FileTreeNode.directoryPath(from: directoryId),
              !items.isEmpty else {
            throw MemoryDirectoryMutationError.invalidDirectory
        }
        guard items.allSatisfy({ canRename($0, inOrgView: inOrgView) }) else {
            throw MemoryDirectoryMutationError.readOnly
        }

        let parent = sourceDirectory.split(separator: "/").dropLast().joined(separator: "/")
        let destinationDirectory = parent.isEmpty ? name : "\(parent)/\(name)"
        guard destinationDirectory != sourceDirectory else {
            throw MemoryDirectoryMutationError.invalidName
        }

        let sourcePaths = Set(items.flatMap { item in
            [item.resource?.document.path, item.draft?.document.path].compactMap { $0 }
        }.map { $0.lowercased() })
        let sourceTreePaths = Set(items.flatMap { item in
            [item.resource?.document.path, item.draft?.document.path]
                .compactMap { $0 }
                .map { FileTreeNode.treePath(for: $0, kind: item.kind) }
        }.map { $0.lowercased() })
        let externalPaths = Set(occupiedPaths.map { $0.lowercased() })
            .subtracting(sourcePaths)
        let externalTreePaths = Set(occupiedTreePaths.map { $0.lowercased() })
            .subtracting(sourceTreePaths)
        var destinationPaths = Set<String>()
        var destinationTreePaths = Set<String>()
        var changes: [MemoryDirectoryRenameChange] = []

        for item in items {
            let treePath = FileTreeNode.treePath(for: item)
            let sourcePrefix = sourceDirectory + "/"
            guard treePath.hasPrefix(sourcePrefix) else {
                throw MemoryDirectoryMutationError.invalidDirectory
            }
            let relativePath = String(treePath.dropFirst(sourcePrefix.count))
            let destinationTreePath = destinationDirectory + "/" + relativePath
            let destinationPath = FileTreeNode.documentPath(
                fromTreePath: destinationTreePath,
                for: item
            )
            let destinationRoot = FileTreeNode.documentPath(
                fromTreePath: destinationDirectory,
                for: item
            )
            let normalizedDestinationRoot = destinationRoot.lowercased()
            let normalizedDestinationDirectory = destinationDirectory.lowercased()
            if containsPathConflict(externalPaths, at: normalizedDestinationRoot)
                || containsPathConflict(
                    externalTreePaths,
                    at: normalizedDestinationDirectory
                )
                || !destinationPaths.insert(destinationPath.lowercased()).inserted
                || !destinationTreePaths.insert(destinationTreePath.lowercased()).inserted {
                throw MemoryDirectoryMutationError.pathCollision(destinationPath)
            }
            changes.append(.init(item: item, newPath: destinationPath))
        }

        return .init(changes: changes.sorted {
            $0.item.document.path.localizedStandardCompare($1.item.document.path)
                == .orderedAscending
        })
    }

    private static func containsPathConflict(_ paths: Set<String>, at root: String) -> Bool {
        let prefix = root + "/"
        return paths.contains {
            $0 == root || $0.hasPrefix(prefix) || root.hasPrefix($0 + "/")
        }
    }

    static func directoryDeletionPlan(
        _ items: [MemoryListItem],
        inOrgView: Bool
    ) -> MemoryDirectoryDeletionPlan? {
        guard !inOrgView, !items.isEmpty else { return nil }
        var itemsToDelete: [MemoryListItem] = []
        var draftsToDiscard: [LocalDraft] = []
        for item in items {
            if canProposeOrganizationDeletion(item, inOrgView: inOrgView) {
                itemsToDelete.append(item)
            } else if item.resource == nil, let draft = item.draft {
                draftsToDiscard.append(draft)
            } else if item.draft?.isDeletion == true {
                continue
            } else {
                return nil
            }
        }
        guard !itemsToDelete.isEmpty || !draftsToDiscard.isEmpty else { return nil }
        return .init(
            itemsToDelete: itemsToDelete,
            draftsToDiscard: draftsToDiscard
        )
    }

    /// New memories are Project-bound proposals for Org authority. The Org
    /// overview is read-only and therefore has no creation scope.
    static func creationScope(inOrgView: Bool) -> MemoryScope? {
        inOrgView ? nil : .org
    }

    /// Org memories that may be added to a project: only in the Org view.
    static func addable(_ items: [MemoryListItem], inOrgView: Bool) -> [MemoryListItem] {
        inOrgView ? items.filter {
            $0.scope == .org && $0.resource != nil && $0.draft?.isDeletion != true
        } : []
    }

    /// Items inherited by the active project that may be removed from it:
    /// only in the Project view.
    static func removable(_ items: [MemoryListItem], inOrgView: Bool) -> [MemoryListItem] {
        inOrgView ? [] : items.filter(\.inherited)
    }

    /// Items that may propose deletion of Org authority. This is the shared
    /// predicate for single-row, batch, and document-toolbar actions.
    static func canProposeOrganizationDeletion(
        _ item: MemoryListItem,
        inOrgView: Bool
    ) -> Bool {
        guard item.resource?.scope == .org,
              item.draft?.isDeletion != true else {
            return false
        }
        return !inOrgView && item.inherited
    }

    static func trashable(_ items: [MemoryListItem], inOrgView: Bool) -> [MemoryListItem] {
        items.filter { canProposeOrganizationDeletion($0, inOrgView: inOrgView) }
    }
}
