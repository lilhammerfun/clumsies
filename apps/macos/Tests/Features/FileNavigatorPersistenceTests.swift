import XCTest
@testable import Clumsies

@MainActor
final class FileNavigatorPersistenceTests: XCTestCase {
    func testRelaunchRestoresTabsModesAndActiveTabPerProjectWithoutReopeningClosedTabs() throws {
        let name = "FileNavigatorTests.\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        let first = workspace(defaults)
        first.navigation.tabs = [tab("a", project: "p1", mode: .source), tab("b", project: "p1"), tab("b", project: "p2")]
        first.navigation.activeTabId = first.navigation.tabs[0].id
        first.context.activeProjectId = "p2"
        first.navigation.activateCurrentProjectTab()
        first.context.activeProjectId = "p1"
        first.navigation.activateCurrentProjectTab()
        XCTAssertEqual(first.navigation.activeVisibleTab?.itemId, "a")

        let restored = workspace(defaults)
        XCTAssertEqual(restored.navigation.tabs.count, 3)
        XCTAssertEqual(restored.navigation.activeVisibleTab?.itemId, "a")
        XCTAssertEqual(restored.navigation.activeVisibleTab?.mode, .source)
        restored.navigation.closeTab(restored.navigation.tabs[1])
        let afterClose = workspace(defaults)
        XCTAssertEqual(afterClose.navigation.visibleTabs.map(\.itemId), ["a"])
        afterClose.context.activeProjectId = "p2"
        afterClose.navigation.activateCurrentProjectTab()
        XCTAssertEqual(afterClose.navigation.activeVisibleTab?.itemId, "b")

        afterClose.navigation.resetAuthority()
        afterClose.context.account = .init(userId: "other", email: "other@test.local", displayName: nil, avatarUrl: nil, role: "member")
        afterClose.navigation.applyWorkspace()
        XCTAssertTrue(afterClose.navigation.tabs.isEmpty)
        XCTAssertEqual(workspace(defaults).navigation.tabs.count, 2)
    }

    func testRestorationWaitsForDraftInventoryBeforeRemovingUnresolvedTabs() throws {
        let name = "FileNavigatorTests.\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        let first = workspace(defaults)
        first.navigation.tabs = [tab("pending-draft", project: "p1")]
        let restored = workspace(defaults, inventoryLoaded: false)
        XCTAssertEqual(restored.navigation.tabs.count, 1)
        restored.navigation.pruneOrphanedMemoryTabs()
        XCTAssertEqual(restored.navigation.tabs.count, 1)
        restored.edits.draftInventoryLoadState = .loaded
        restored.navigation.pruneOrphanedMemoryTabs()
        XCTAssertTrue(restored.navigation.tabs.isEmpty)
    }

    private func tab(_ id: String, project: String, mode: WorkbenchTabMode = .preview) -> WorkbenchTab {
        .init(section: .memory, projectId: project, itemId: id, mode: mode, title: id)
    }

    private func workspace(_ defaults: UserDefaults, inventoryLoaded: Bool = true) -> WorkspaceCoordinator {
        let workspace = WorkspaceCoordinator(navigationDefaults: defaults)
        workspace.context.account = .init(userId: "owner", email: "owner@test.local", displayName: nil, avatarUrl: nil, role: "owner")
        workspace.context.organization = .init(orgId: "org", name: "Test")
        workspace.context.projects = ["p1", "p2"].map {
            .init(id: $0, name: $0, refCommitId: nil, refEtag: "", selectedOrgResourceIds: ["a", "b"], orgSelectionRevision: 1, isLoaded: true)
        }
        workspace.context.activeProjectId = "p1"
        workspace.context.phase = .ready
        workspace.catalog.resources = ["a", "b"].map {
            .init(id: $0, scope: .org, projectId: nil, projectName: nil, kind: .context,
                  contentHash: "hash", updatedAt: "", refCommitId: nil, contentLoaded: true,
                  document: .init(title: $0, path: $0 + ".md", body: "content"))
        }
        workspace.edits.draftInventoryLoadState = inventoryLoaded ? .loaded : .loading
        workspace.navigation.applyWorkspace()
        return workspace
    }
}
