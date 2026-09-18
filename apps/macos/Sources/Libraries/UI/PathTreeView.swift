import SwiftUI

struct PathTreeItem: Identifiable, Hashable, Sendable {
    let id: String
    let path: String
    let fallbackName: String?

    init(id: String, path: String, fallbackName: String? = nil) {
        self.id = id
        self.path = path
        self.fallbackName = fallbackName
    }
}

struct VisiblePathTreeNode: Identifiable, Sendable {
    let node: PathTreeNode
    let depth: Int

    var id: String { node.id }
}

struct PathTreeNode: Identifiable, Sendable {
    let id: String
    let name: String
    let item: PathTreeItem?
    let children: [PathTreeNode]?

    private struct Entry {
        let item: PathTreeItem
        let components: ArraySlice<String>
    }

    static func build(_ items: [PathTreeItem]) -> [PathTreeNode] {
        let entries = items.map { item in
            let pathComponents = item.path.split(separator: "/").map(String.init)
            let components = pathComponents.isEmpty
                ? [item.fallbackName ?? "Untitled"]
                : pathComponents
            return Entry(item: item, components: components[...])
        }
        return build(entries, prefix: "")
    }

    private static func build(_ entries: [Entry], prefix: String) -> [PathTreeNode] {
        let groups = Dictionary(grouping: entries) { $0.components.first ?? "Untitled" }
        return groups.keys.sorted {
            $0.localizedStandardCompare($1) == .orderedAscending
        }.map { name in
            let group = groups[name] ?? []
            if let file = group.first(where: { $0.components.count <= 1 }) {
                return .init(
                    id: file.item.id,
                    name: name,
                    item: file.item,
                    children: nil
                )
            }

            let path = prefix.isEmpty ? name : "\(prefix)/\(name)"
            let descendants = group.map {
                Entry(item: $0.item, components: $0.components.dropFirst())
            }
            return .init(
                id: "directory:\(path)",
                name: name,
                item: nil,
                children: build(descendants, prefix: path)
            )
        }
    }

    static func directoryIds(in nodes: [PathTreeNode]) -> Set<String> {
        nodes.reduce(into: Set<String>()) { result, node in
            guard let children = node.children else { return }
            result.insert(node.id)
            result.formUnion(directoryIds(in: children))
        }
    }

    static func allIds(in nodes: [PathTreeNode]) -> [String] {
        nodes.flatMap { node in
            [node.id] + (node.children.map { allIds(in: $0) } ?? [])
        }
    }

    static func node(withId id: String, in nodes: [PathTreeNode]) -> PathTreeNode? {
        for node in nodes {
            if node.id == id { return node }
            if let children = node.children,
               let match = self.node(withId: id, in: children) {
                return match
            }
        }
        return nil
    }

    static func firstItemId(in nodes: [PathTreeNode]) -> String? {
        for node in nodes {
            if let item = node.item { return item.id }
            if let children = node.children,
               let itemId = firstItemId(in: children) {
                return itemId
            }
        }
        return nil
    }

    static func visibleNodes(
        _ nodes: [PathTreeNode],
        expandedDirectoryIds: Set<String>,
        depth: Int = 0
    ) -> [VisiblePathTreeNode] {
        nodes.flatMap { node in
            var result = [VisiblePathTreeNode(node: node, depth: depth)]
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

struct PathTreeView: View {
    let items: [PathTreeItem]
    @Binding var selection: String?

    @State private var expandedDirectoryIds: Set<String> = []
    @State private var initialized = false

    private var roots: [PathTreeNode] {
        PathTreeNode.build(items)
    }

    private var visibleNodes: [VisiblePathTreeNode] {
        PathTreeNode.visibleNodes(roots, expandedDirectoryIds: expandedDirectoryIds)
    }

    var body: some View {
        List(selection: $selection) {
            ForEach(visibleNodes) { entry in
                if let item = entry.node.item {
                    PathTreeRowLabel(
                        name: entry.node.name,
                        path: item.path,
                        depth: entry.depth,
                        isDirectory: false,
                        isExpanded: false
                    )
                    .tag(item.id)
                    .accessibilityIdentifier("path-tree-item-\(item.id)")
                    .listRowInsets(.init(top: 0, leading: 5, bottom: 0, trailing: 5))
                    .listRowSeparator(.hidden)
                } else {
                    Button {
                        toggleDirectory(entry.id)
                    } label: {
                        PathTreeRowLabel(
                            name: entry.node.name,
                            path: nil,
                            depth: entry.depth,
                            isDirectory: true,
                            isExpanded: expandedDirectoryIds.contains(entry.id)
                        )
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("path-tree-directory-\(entry.id)")
                    .listRowInsets(.init(top: 0, leading: 5, bottom: 0, trailing: 5))
                    .listRowSeparator(.hidden)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color(nsColor: .controlBackgroundColor))
        .onAppear(perform: initialize)
        .onChange(of: items) { _, _ in synchronizeWithItems() }
    }

    private func initialize() {
        guard !initialized else { return }
        initialized = true
        expandedDirectoryIds = PathTreeNode.directoryIds(in: roots)
        ensureSelection()
    }

    private func synchronizeWithItems() {
        expandedDirectoryIds.formUnion(PathTreeNode.directoryIds(in: roots))
        ensureSelection()
    }

    private func ensureSelection() {
        let itemIds = Set(items.map(\.id))
        if selection == nil || !itemIds.contains(selection ?? "") {
            selection = PathTreeNode.firstItemId(in: roots)
        }
    }

    private func toggleDirectory(_ id: String) {
        withAnimation(.snappy(duration: 0.14)) {
            if expandedDirectoryIds.contains(id) {
                expandedDirectoryIds.remove(id)
            } else {
                expandedDirectoryIds.insert(id)
            }
        }
    }
}

struct PathTreeRowLabel<Accessory: View>: View {
    let name: String
    let path: String?
    let depth: Int
    let isDirectory: Bool
    let isExpanded: Bool
    let titleColor: Color
    private let accessory: Accessory

    init(
        name: String,
        path: String?,
        depth: Int,
        isDirectory: Bool,
        isExpanded: Bool,
        titleColor: Color = .primary,
        @ViewBuilder accessory: () -> Accessory
    ) {
        self.name = name
        self.path = path
        self.depth = depth
        self.isDirectory = isDirectory
        self.isExpanded = isExpanded
        self.titleColor = titleColor
        self.accessory = accessory()
    }

    var body: some View {
        HStack(spacing: 6) {
            if isDirectory {
                Image(systemName: isExpanded ? "folder.fill" : "folder")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .frame(width: 14)
            } else {
                FileSymbolView(path: path ?? name)
            }

            Text(name)
                .font(.system(size: 12.5, weight: .regular))
                .lineLimit(1)
                .truncationMode(.middle)
                .foregroundStyle(titleColor)

            Spacer(minLength: 4)
            accessory
        }
        .padding(.leading, CGFloat(depth) * 13 + 5)
        .padding(.trailing, 7)
        .frame(maxWidth: .infinity, minHeight: 25, maxHeight: 25, alignment: .leading)
        .contentShape(Rectangle())
        .help(path ?? name)
    }
}

extension PathTreeRowLabel where Accessory == EmptyView {
    init(
        name: String,
        path: String?,
        depth: Int,
        isDirectory: Bool,
        isExpanded: Bool,
        titleColor: Color = .primary
    ) {
        self.init(
            name: name,
            path: path,
            depth: depth,
            isDirectory: isDirectory,
            isExpanded: isExpanded,
            titleColor: titleColor
        ) {
            EmptyView()
        }
    }
}
