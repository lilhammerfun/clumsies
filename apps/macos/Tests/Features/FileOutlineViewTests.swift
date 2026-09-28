import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class FileOutlineViewTests: XCTestCase {
    private var selection: Set<String> = []
    private var opened: [String] = []
    private let persistenceKey = "FileOutlineTests.\(UUID())"

    private func tree(_ items: [MemoryListItem], filtering: Bool = false) -> FileOutlineView<Text> {
        FileOutlineView(
            roots: FileTreeNode.build(items),
            selection: Binding(get: { self.selection }, set: { self.selection = $0 }),
            persistenceKey: persistenceKey,
            isFiltering: filtering,
            onOpen: { self.opened.append($0) },
            onRename: { _ in }
        ) { node, _, _, _ in Text(node.name) }
    }

    func testClosedDocumentCanBeOpenedAgainWithoutChangingTreeSelection() throws {
        let item = memory("a", "a.md")
        let view = tree([item])
        let coordinator = view.makeCoordinator()
        coordinator.update(view)
        coordinator.outline.selectRowIndexes([0], byExtendingSelection: false)
        XCTAssertEqual(selection, ["a"])
        XCTAssertTrue(opened.isEmpty, "Selection alone is not an open command.")
        coordinator.activateRow(0, modifiers: [])
        coordinator.activateRow(0, modifiers: [])
        XCTAssertEqual(opened, ["a", "a"], "A second click must still issue an open command.")

        let workspace = WorkspaceCoordinator()
        workspace.context.phase = .ready
        workspace.catalog.resources = [try XCTUnwrap(item.resource)]
        workspace.navigation.open(item)
        XCTAssertTrue(workspace.navigation.closeActiveTab())
        XCTAssertTrue(workspace.navigation.visibleTabs.isEmpty)
        workspace.navigation.open(item)
        XCTAssertEqual(workspace.navigation.visibleTabs.map(\.itemId), ["a"])
    }

    func testNativeMultipleSelectionDoesNotOpenFiles() {
        let view = tree([memory("a", "a.md"), memory("b", "b.md")])
        let coordinator = view.makeCoordinator()
        coordinator.update(view)
        coordinator.outline.selectRowIndexes([0, 1], byExtendingSelection: false)
        coordinator.activateRow(1, modifiers: .command)
        XCTAssertEqual(selection, ["a", "b"])
        XCTAssertTrue(opened.isEmpty)
    }

    func testFoldersToggleOnClickWithoutTrianglesAndRetainKeyboardNavigation() throws {
        let view = tree([memory("a", "notes/a.md")])
        let coordinator = view.makeCoordinator()
        coordinator.update(view)
        let outline = coordinator.outline
        outline.frame = NSRect(x: 0, y: 0, width: 300, height: 200)
        let folder = try XCTUnwrap(outline.item(atRow: 0))
        outline.selectRowIndexes([0], byExtendingSelection: false)
        XCTAssertEqual(outline.frameOfOutlineCell(atRow: 0), .zero)
        XCTAssertEqual(outline.frameOfCell(atColumn: 0, row: 0).minX, 0)
        for modifier: NSEvent.ModifierFlags in [.command, .shift, .control, .option] {
            coordinator.activateRow(0, modifiers: modifier)
            XCTAssertFalse(outline.isItemExpanded(folder))
        }
        coordinator.activateRow(0, modifiers: [])
        XCTAssertTrue(outline.isItemExpanded(folder))
        XCTAssertEqual(outline.frameOfCell(atColumn: 0, row: 1).minX, outline.indentationPerLevel)
        outline.selectRowIndexes([0, 1], byExtendingSelection: false)
        coordinator.activateRow(0, modifiers: [])
        XCTAssertTrue(outline.isItemExpanded(folder), "A multi-selection must not toggle folders.")
        outline.selectRowIndexes([0], byExtendingSelection: false)
        NSApp.sendAction(try XCTUnwrap(outline.doubleAction), to: coordinator, from: outline)
        XCTAssertTrue(outline.isItemExpanded(folder), "A double-click must not undo the first click.")
        coordinator.activateRow(0, modifiers: [])
        XCTAssertFalse(outline.isItemExpanded(folder))

        for (code, character, expanded) in [(UInt16(124), "\u{F703}", true), (UInt16(123), "\u{F702}", false)] {
            let event = try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                windowNumber: 0, context: nil, characters: character,
                charactersIgnoringModifiers: character, isARepeat: false, keyCode: code
            ))
            outline.keyDown(with: event)
            XCTAssertEqual(outline.isItemExpanded(folder), expanded)
        }
        XCTAssertTrue(opened.isEmpty)
    }

    func testReloadKeepsStableNodesAndDoesNotExpandCollapsedDirectories() throws {
        let view = tree([memory("a", "notes/a.md")])
        let coordinator = view.makeCoordinator()
        coordinator.update(view)
        let folder = try XCTUnwrap(coordinator.outline.item(atRow: 0) as? NSObject)
        coordinator.outline.expandItem(folder)
        coordinator.outline.collapseItem(folder)
        coordinator.update(tree([memory("a", "notes/renamed.md"), memory("b", "notes/b.md")]))
        XCTAssertTrue(folder === coordinator.outline.item(atRow: 0) as? NSObject)
        XCTAssertFalse(coordinator.outline.isItemExpanded(folder))
        XCTAssertEqual(coordinator.outline.numberOfRows, 1)
    }

    func testSearchTemporarilyExpandsResultsAndRestoresPreviousDisclosureState() throws {
        let items = [memory("a", "notes/a.md"), memory("b", "other/b.md")]
        let view = tree(items)
        let coordinator = view.makeCoordinator()
        coordinator.update(view)
        let notes = try XCTUnwrap(coordinator.outline.item(atRow: 0))
        coordinator.outline.expandItem(notes)
        coordinator.update(tree([], filtering: true))
        XCTAssertEqual(coordinator.outline.numberOfRows, 0)
        coordinator.update(tree([items[1]], filtering: true))
        XCTAssertEqual(coordinator.outline.numberOfRows, 2)
        coordinator.update(tree(items))
        XCTAssertEqual(coordinator.outline.numberOfRows, 3)
        XCTAssertTrue(coordinator.outline.isItemExpanded(coordinator.outline.item(atRow: 0)))
        XCTAssertFalse(coordinator.outline.isItemExpanded(coordinator.outline.item(atRow: 2)))
        XCTAssertTrue(opened.isEmpty)
    }

    func testProgrammaticTabSelectionRevealsOnlyItsAncestorsWithoutOpeningAgain() {
        let view = tree([memory("a", "notes/nested/a.md"), memory("b", "other/b.md")])
        let coordinator = view.makeCoordinator()
        coordinator.update(view)
        selection = ["a"]
        coordinator.update(view)
        XCTAssertEqual(coordinator.outline.numberOfRows, 4)
        XCTAssertEqual(coordinator.outline.selectedRow, 2)
        XCTAssertTrue(opened.isEmpty)
    }

    func testDisclosureStateSurvivesRecreatingTheNavigator() throws {
        let view = tree([memory("a", "notes/a.md")])
        let first = view.makeCoordinator()
        first.update(view)
        first.outline.expandItem(try XCTUnwrap(first.outline.item(atRow: 0)))
        let restored = view.makeCoordinator()
        restored.update(view)
        XCTAssertEqual(restored.outline.numberOfRows, 2)
    }

    private func memory(_ id: String, _ path: String) -> MemoryListItem {
        .init(id: id, resource: .init(
            id: id, scope: .org, projectId: nil, projectName: nil, kind: .context,
            contentHash: "sha256:\(id)", updatedAt: "2026-09-24T00:00:00Z", refCommitId: "base",
            contentLoaded: true, document: .init(title: id, path: path, body: "# \(id)\n")
        ), draft: nil, inherited: false)
    }
}
