import AppKit
import SwiftUI

/// AppKit owns selection, disclosure and keyboard navigation. Callers provide
/// row presentation and document commands; selecting a row does not open it.
struct FileOutlineView<Row: View>: NSViewRepresentable {
    let roots: [FileTreeNode]
    @Binding var selection: Set<String>
    let persistenceKey: String
    let isFiltering: Bool
    let onOpen: (String) -> Void
    let onRename: (String) -> Void
    var canMove: (Set<String>, String?) -> Bool = { _, _ in false }
    var onMove: (Set<String>, String?) -> Void = { _, _ in }
    @ViewBuilder let row: (FileTreeNode, Bool, Bool, Set<String>) -> Row

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.documentView = context.coordinator.outline
        context.coordinator.update(self)
        return scroll
    }

    func updateNSView(_ view: NSScrollView, context: Context) {
        context.coordinator.update(self)
    }

    @MainActor
    final class Coordinator: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
        // NSOutlineView requires stable object identities across reloads.
        final class Node: NSObject {
            var value: FileTreeNode
            var children: [Node] = []
            weak var parent: Node?
            init(_ value: FileTreeNode) { self.value = value }
        }

        let outline = FileOutlineControl()
        private var parent: FileOutlineView
        private var nodes: [String: Node] = [:]
        private var roots: [Node] = []
        private var previousRoots: [FileTreeNode]?
        private var selection: Set<String> = []
        private var expansionBeforeFiltering: Set<String>?
        private var isUpdating = false
        private let dragType = NSPasteboard.PasteboardType("ai.clumsies.file-nodes")

        init(_ parent: FileOutlineView) {
            self.parent = parent
            super.init()
            let column = NSTableColumn(identifier: .init("file"))
            outline.addTableColumn(column)
            outline.outlineTableColumn = column
            outline.headerView = nil
            outline.style = .plain
            outline.backgroundColor = .controlBackgroundColor
            outline.rowHeight = 25
            outline.intercellSpacing = .zero
            outline.indentationPerLevel = 14
            outline.allowsMultipleSelection = true
            outline.allowsEmptySelection = true
            outline.registerForDraggedTypes([dragType])
            outline.setDraggingSourceOperationMask(.move, forLocal: true)
            outline.setDraggingSourceOperationMask([], forLocal: false)
            outline.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
            outline.dataSource = self
            outline.delegate = self
            outline.target = self
            outline.action = #selector(clicked)
            outline.doubleAction = #selector(doubleClicked)
            outline.openSelection = { [weak self] in self?.openSelection() }
            outline.renameSelection = { [weak self] in
                guard let self, self.selection.count == 1, let id = self.selection.first else { return }
                self.parent.onRename(id)
            }
            outline.setAccessibilityLabel(String(localized: "Files"))
            outline.setAccessibilityIdentifier("memory-file-outline")
        }

        func update(_ parent: FileOutlineView) {
            isUpdating = true
            defer { isUpdating = false }
            self.parent = parent
            let enteringFilter = parent.isFiltering && expansionBeforeFiltering == nil
            let leavingFilter = !parent.isFiltering && expansionBeforeFiltering != nil
            if enteringFilter {
                expansionBeforeFiltering = expandedIDs
                outline.autosaveExpandedItems = false
            }
            if previousRoots != parent.roots {
                let expanded = expandedIDs
                var updatedNodes: [String: Node] = [:]
                func install(_ value: FileTreeNode, parent: Node? = nil) -> Node {
                    let node = nodes[value.id] ?? Node(value)
                    updatedNodes[value.id] = node
                    node.value = value
                    node.parent = parent
                    node.children = (value.children ?? []).map { install($0, parent: node) }
                    return node
                }
                roots = parent.roots.map { install($0) }
                nodes = updatedNodes
                if outline.autosaveName == nil {
                    outline.autosaveName = parent.persistenceKey
                    outline.autosaveExpandedItems = !parent.isFiltering
                }
                outline.reloadData()
                expand(expanded)
                previousRoots = parent.roots
            }
            if parent.isFiltering {
                outline.expandItem(nil, expandChildren: true)
            } else if leavingFilter {
                outline.collapseItem(nil, collapseChildren: true)
                expand(expansionBeforeFiltering ?? [])
                expansionBeforeFiltering = nil
                outline.autosaveExpandedItems = true
            }
            // A tab change may reveal its file. Ordinary refreshes must not
            // undo a folder collapse or scroll the navigator back to the editor.
            if selection != parent.selection, parent.selection.count == 1,
               let id = parent.selection.first, let node = nodes[id] {
                revealAncestors(of: node)
            }
            selection = parent.selection
            let indexes = IndexSet((0..<outline.numberOfRows).filter {
                guard let node = outline.item(atRow: $0) as? Node else { return false }
                return selection.contains(node.value.id)
            })
            outline.selectRowIndexes(indexes, byExtendingSelection: false)
            refreshRows()
        }

        private var expandedIDs: Set<String> {
            Set(nodes.values.filter { outline.isItemExpanded($0) }.map { $0.value.id })
        }

        private func expand(_ ids: Set<String>) {
            func visit(_ node: Node) {
                if ids.contains(node.value.id) { outline.expandItem(node) }
                node.children.forEach(visit)
            }
            roots.forEach(visit)
        }

        private func revealAncestors(of node: Node) {
            // Parentage comes from the displayed tree, including filtered results.
            if let parent = node.parent {
                revealAncestors(of: parent)
                outline.expandItem(parent)
            }
        }

        private func content(for node: Node) -> Row {
            let id = node.value.id
            return parent.row(node.value, outline.isItemExpanded(node), selection.contains(id),
                              selection.contains(id) ? selection : [id])
        }

        private func refreshRows() {
            for index in 0..<outline.numberOfRows {
                guard let node = outline.item(atRow: index) as? Node,
                      let cell = outline.view(atColumn: 0, row: index, makeIfNecessary: false)
                        as? FileOutlineCell<Row> else { continue }
                cell.rootView = content(for: node)
            }
        }

        func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
            (item as? Node)?.children.count ?? roots.count
        }

        func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
            ((item as? Node)?.children ?? roots)[index]
        }

        func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
            (item as? Node)?.value.children != nil
        }

        func outlineView(_ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any) -> NSView? {
            guard let node = item as? Node else { return nil }
            let identifier = NSUserInterfaceItemIdentifier("file-row")
            let cell = outlineView.makeView(withIdentifier: identifier, owner: nil)
                as? FileOutlineCell<Row> ?? FileOutlineCell(rootView: content(for: node))
            cell.identifier = identifier
            cell.rootView = content(for: node)
            cell.toolTip = node.value.item?.document.path ?? node.value.name
            return cell
        }

        func outlineView(_ outlineView: NSOutlineView, persistentObjectForItem item: Any?) -> Any? {
            (item as? Node)?.value.id
        }

        func outlineView(_ outlineView: NSOutlineView, pasteboardWriterForItem item: Any) -> NSPasteboardWriting? {
            guard let node = item as? Node else { return nil }
            let writer = NSPasteboardItem()
            writer.setString(node.value.id, forType: dragType)
            return writer
        }

        private func draggedIDs(_ info: NSDraggingInfo) -> Set<String> {
            guard (info.draggingSource as? NSOutlineView) === outline else { return [] }
            return Set((info.draggingPasteboard.pasteboardItems ?? []).compactMap { $0.string(forType: dragType) })
        }

        func outlineView(_ outlineView: NSOutlineView, validateDrop info: NSDraggingInfo,
                         proposedItem item: Any?, proposedChildIndex index: Int) -> NSDragOperation {
            let target = item as? Node
            guard target == nil || target?.value.children != nil,
                  parent.canMove(draggedIDs(info), target?.value.id) else { return [] }
            outlineView.setDropItem(item, dropChildIndex: NSOutlineViewDropOnItemIndex)
            return .move
        }

        func outlineView(_ outlineView: NSOutlineView, acceptDrop info: NSDraggingInfo,
                         item: Any?, childIndex index: Int) -> Bool {
            let ids = draggedIDs(info)
            let destination = (item as? Node)?.value.id
            guard parent.canMove(ids, destination) else { return false }
            parent.onMove(ids, destination)
            return true
        }

        func outlineView(_ outlineView: NSOutlineView, itemForPersistentObject object: Any) -> Any? {
            (object as? String).flatMap { nodes[$0] }
        }

        func outlineViewSelectionDidChange(_ notification: Notification) {
            guard !isUpdating else { return }
            selection = Set(outline.selectedRowIndexes.compactMap {
                (outline.item(atRow: $0) as? Node)?.value.id
            })
            parent.selection = selection
            refreshRows()
        }

        func outlineViewItemDidExpand(_ notification: Notification) { refreshRows() }
        func outlineViewItemDidCollapse(_ notification: Notification) { refreshRows() }

        private func openSelection() {
            guard selection.count == 1, let id = selection.first,
                  nodes[id]?.value.item != nil, nodes[id]?.value.children == nil else { return }
            parent.onOpen(id)
        }

        @objc private func clicked() {
            activateRow(outline.clickedRow, modifiers: NSApp.currentEvent?.modifierFlags ?? [])
        }

        func activateRow(_ row: Int, modifiers: NSEvent.ModifierFlags) {
            guard modifiers.intersection([.command, .shift, .control, .option]).isEmpty,
                  row >= 0, outline.selectedRowIndexes.contains(row) else { return }
            openSelection()
        }

        @objc private func doubleClicked() {
            guard let node = outline.item(atRow: outline.clickedRow) as? Node else { return }
            if node.value.children != nil {
                if outline.isItemExpanded(node) { outline.collapseItem(node) }
                else { outline.expandItem(node) }
            } else {
                openSelection()
            }
        }
    }
}

@MainActor
final class FileOutlineControl: NSOutlineView {
    var openSelection: (() -> Void)?
    var renameSelection: (() -> Void)?

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 125, event.modifierFlags.contains(.command) {
            openSelection?()
        } else if event.keyCode == 36, event.modifierFlags.intersection(.deviceIndependentFlagsMask).isEmpty {
            renameSelection?()
        } else {
            super.keyDown(with: event)
        }
    }
}

@MainActor
private final class FileOutlineCell<Content: View>: NSHostingView<Content> {
    // SwiftUI supplies the row and context menu; AppKit handles row selection.
    override func mouseDown(with event: NSEvent) { superview?.mouseDown(with: event) }
}
