import AppKit
import SwiftUI

struct FileTreeNode: Identifiable, Equatable {
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
                fallbackName: item.document.title,
                isDirectory: item.document.isDirectory
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
              node.children != nil else {
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

enum MemoryDirectoryMutationError: UserFacingError, Equatable {
    case invalidDirectory
    case invalidName
    case readOnly
    case pathCollision(String)
    case invalidDestination

    var errorDescription: String? {
        switch self {
        case .invalidDirectory:
            String(localized: "This folder no longer exists.")
        case .invalidName:
            String(localized: "Choose a different folder name without a slash.")
        case .readOnly:
            String(localized: "Every item in the folder must be editable before the folder can be changed.")
        case .pathCollision(let path):
            String(localized: "The destination already contains \(path).")
        case .invalidDestination:
            String(localized: "Choose a different destination outside the selected folders.")
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
        !drafts.isEmpty && Set(drafts.map(\.scope)).count == 1 && drafts.allSatisfy {
            $0.syncStatus == .synced && $0.serverId != nil
        }
    }

    /// One directory Review contains open Drafts with the same owner below the
    /// selection. Unchanged files are excluded.
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

    /// Rename is a Project proposal by default. Project views only
    /// expose it for Org resources that are still selected by that project.
    /// The Org overview is authority-only/read-only because it has no
    /// unambiguous Project carrier for a LocalDraft.
    /// Pure create Drafts keep their full local document and can be renamed;
    /// published Project Memory supports the same proposal actions.
    static func canRename(_ item: MemoryListItem, inOrgView: Bool) -> Bool {
        guard !inOrgView, item.draft?.isDeletion != true else {
            return false
        }
        if item.resource?.scope == .org { return item.inherited }
        return item.scope == .project || (item.resource == nil && item.draft?.targetId == nil)
    }

    static func directoryRenamePlan(
        directoryId: String,
        newName: String,
        items: [MemoryListItem],
        occupiedPaths: Set<String>,
        occupiedTreePaths: Set<String>,
        inOrgView: Bool,
        destinationParent: String? = nil,
        directoryPaths: Set<String> = [], directoryTreePaths: Set<String> = []
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

        let parent = destinationParent ?? sourceDirectory.split(separator: "/").dropLast().joined(separator: "/")
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
            guard treePath.hasPrefix(sourcePrefix) || (item.document.isDirectory && treePath == sourceDirectory) else {
                throw MemoryDirectoryMutationError.invalidDirectory
            }
            let relativePath = String(treePath.dropFirst(sourcePrefix.count))
            let destinationTreePath = treePath == sourceDirectory ? destinationDirectory : destinationDirectory + "/" + relativePath
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
            if containsPathConflict(externalPaths, at: normalizedDestinationRoot, directories: directoryPaths)
                || containsPathConflict(
                    externalTreePaths,
                    at: normalizedDestinationDirectory, directories: directoryTreePaths
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

    private static func containsPathConflict(_ paths: Set<String>, at root: String, directories: Set<String> = []) -> Bool {
        let directories = Set(directories.map { $0.lowercased() })
        let prefix = root + "/"
        return paths.contains {
            $0 == root || $0.hasPrefix(prefix) || (!directories.contains($0) && root.hasPrefix($0 + "/"))
        }
    }

    static func movePlan(
        selectedNodeIds: Set<String>, to directoryId: String?, roots: [FileTreeNode],
        occupiedPaths: Set<String>, occupiedTreePaths: Set<String>, inOrgView: Bool,
        directoryPaths: Set<String> = [], directoryTreePaths: Set<String> = []
    ) throws -> MemoryDirectoryRenamePlan {
        let destination: String
        if let directoryId {
            guard let node = FileTreeNode.node(withId: directoryId, in: roots), node.children != nil,
                  let path = FileTreeNode.directoryPath(from: directoryId) else {
                throw MemoryDirectoryMutationError.invalidDestination
            }
            destination = path
        } else {
            destination = ""
        }
        // Selecting a folder and one of its children moves that child once.
        func topLevelSelection(_ nodes: [FileTreeNode]) -> [FileTreeNode] {
            nodes.flatMap { node in
                selectedNodeIds.contains(node.id) ? [node] : topLevelSelection(node.children ?? [])
            }
        }
        let selected = topLevelSelection(roots)
        guard !selected.isEmpty, selectedNodeIds.isSubset(of: Set(FileTreeNode.allIds(in: roots))) else {
            throw MemoryDirectoryMutationError.invalidDestination
        }
        var changes: [MemoryDirectoryRenameChange] = []
        var destinationRoots = Set<String>()
        for node in selected {
            guard let source = node.item.map({ FileTreeNode.treePath(for: $0) })
                ?? FileTreeNode.directoryPath(from: node.id) else {
                throw MemoryDirectoryMutationError.invalidDestination
            }
            if node.children != nil,
               destination.lowercased() == source.lowercased()
                || destination.lowercased().hasPrefix(source.lowercased() + "/") {
                throw MemoryDirectoryMutationError.invalidDestination
            }
            let target = destination.isEmpty ? node.name : destination + "/" + node.name
            if source == target { continue }
            if containsPathConflict(destinationRoots, at: target.lowercased()) {
                throw MemoryDirectoryMutationError.pathCollision(target)
            }
            destinationRoots.insert(target.lowercased())
            if let item = node.item, node.children == nil {
                guard canRename(item, inOrgView: inOrgView) else {
                    throw MemoryDirectoryMutationError.readOnly
                }
                let path = FileTreeNode.documentPath(fromTreePath: target, for: item)
                let sourcePaths = [item.resource?.document.path, item.draft?.document.path].compactMap { $0 }
                let externalPaths = Set(occupiedPaths.map { $0.lowercased() })
                    .subtracting(sourcePaths.map { $0.lowercased() })
                let externalTreePaths = Set(occupiedTreePaths.map { $0.lowercased() })
                    .subtracting(sourcePaths.map { FileTreeNode.treePath(for: $0, kind: item.kind).lowercased() })
                guard !containsPathConflict(externalPaths, at: path.lowercased(), directories: directoryPaths),
                      !containsPathConflict(externalTreePaths, at: target.lowercased(), directories: directoryTreePaths) else {
                    throw MemoryDirectoryMutationError.pathCollision(path)
                }
                changes.append(.init(item: item, newPath: path))
            } else {
                let plan = try directoryRenamePlan(
                    directoryId: node.id, newName: node.name,
                    items: FileTreeNode.items(in: roots, selectedNodeIds: [node.id]),
                    occupiedPaths: occupiedPaths, occupiedTreePaths: occupiedTreePaths,
                    inOrgView: inOrgView, destinationParent: destination,
                    directoryPaths: directoryPaths, directoryTreePaths: directoryTreePaths
                )
                changes.append(contentsOf: plan.changes)
            }
        }
        guard !changes.isEmpty else { throw MemoryDirectoryMutationError.invalidDestination }
        return .init(changes: changes)
    }

    static func directoryDeletionPlan(
        _ items: [MemoryListItem],
        inOrgView: Bool
    ) -> MemoryDirectoryDeletionPlan? {
        guard !inOrgView, !items.isEmpty else { return nil }
        var itemsToDelete: [MemoryListItem] = []
        var draftsToDiscard: [LocalDraft] = []
        for item in items {
            if canProposeMemoryDeletion(item, inOrgView: inOrgView) {
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
        inOrgView ? nil : .project
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
    static func canProposeMemoryDeletion(
        _ item: MemoryListItem,
        inOrgView: Bool
    ) -> Bool {
        guard !inOrgView, item.draft?.isDeletion != true else { return false }
        return item.resource?.scope == .project || (item.draft?.scope == .org && item.draft?.targetId != nil)

    }

    static func trashable(_ items: [MemoryListItem], inOrgView: Bool) -> [MemoryListItem] {
        items.filter { canProposeMemoryDeletion($0, inOrgView: inOrgView) }
    }
}
