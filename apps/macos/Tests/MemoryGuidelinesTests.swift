import XCTest
@testable import Clumsies

final class MemoryGuidelinesTests: XCTestCase {
    func testEmptySpaceOffersBundledDocumentAtExactDefaultPath() throws {
        let setup = try plan()
        XCTAssertEqual(setup.action, .createDefault)
        let document = try MemoryGuidelines.defaultDocument()
        XCTAssertEqual(document.path, setup.path)
        XCTAssertFalse(document.body.isEmpty)
        XCTAssertTrue(document.body.contains("https://"))
    }

    func testExistingCustomDocumentIsOpenedWithoutSeedingDefaults() throws {
        let custom = resource(path: "team/MEMORY.md")
        let item = MemoryListItem(id: custom.id, resource: custom, draft: nil, inherited: true)
        XCTAssertEqual(
            try plan(path: custom.document.path, items: [item], resources: [custom]).action,
            .open(custom.id)
        )
        XCTAssertThrowsError(try plan(path: "missing/custom.md"))
        XCTAssertEqual(MemoryGuidelines.configuredPath("  team/MEMORY.md\n"), "team/MEMORY.md")
        XCTAssertEqual(MemoryGuidelines.configuredPath(nil), MemoryGuidelines.defaultPath)
        XCTAssertEqual(MemoryGuidelines.configuredPath("  "), MemoryGuidelines.defaultPath)
    }

    func testOrganizationGuidelinesAreSelectedAndChangedDestinationNeedsAnotherChoice() throws {
        let shared = resource()
        let empty = try plan()
        let existing = try plan(resources: [shared])
        XCTAssertEqual(existing.action, .useOrganization(shared))
        XCTAssertFalse(existing.hasSameDestination(as: empty))
        XCTAssertTrue(existing.hasSameDestination(as: try plan(resources: [shared])))
        let anotherProject = MemoryGuidelinesSetup(projectId: "other", path: empty.path, action: empty.action)
        XCTAssertFalse(empty.hasSameDestination(as: anotherProject))
        let custom = resource(path: "team/custom.md")
        XCTAssertEqual(try plan(path: custom.document.path, resources: [custom]).action, .useOrganization(custom))
    }

    func testExistingDraftIsReusedAndPendingDeletionOrRenameBlocksInitialization() throws {
        let item = draftItem()
        XCTAssertEqual(try plan(items: [item]).action, .open(item.id))
        XCTAssertThrowsError(try plan(items: [draftItem(isDeletion: true)]))
        let shared = resource()
        let renamed = draftItem(path: "moved.md", targetId: shared.id)
        XCTAssertThrowsError(try plan(items: [renamed], resources: [shared]))
    }

    private func plan(
        path: String = MemoryGuidelines.defaultPath,
        items: [MemoryListItem] = [],
        resources: [MemoryResource] = []
    ) throws -> MemoryGuidelinesSetup {
        try MemoryGuidelines.setup(projectId: "project", path: path, items: items, organizationResources: resources)
    }

    private func resource(path: String = MemoryGuidelines.defaultPath) -> MemoryResource {
        .init(
            id: "shared", scope: .org, projectId: nil, projectName: nil, kind: .context,
            contentHash: "hash", updatedAt: "2026-09-17", refCommitId: "commit", contentLoaded: true,
            document: .init(title: "Team guidelines", path: path, body: "Custom team guidance")
        )
    }

    private func draftItem(
        path: String = MemoryGuidelines.defaultPath,
        targetId: String? = nil,
        isDeletion: Bool = false
    ) -> MemoryListItem {
        let draft = LocalDraft(
            id: "draft", projectId: "project", serverId: nil, serverVersion: 0,
            baseCommitId: nil, currentCommitId: nil, freshness: .current,
            hasUpstreamResourceChanges: false, reconciliation: .clean, reconciliationCandidateId: nil,
            scope: .org, kind: .context, targetId: targetId, status: .open, origin: .desktop,
            syncStatus: .queued, updatedAt: "2026-09-17",
            document: .init(title: "Custom draft", path: path, body: "Keep my edits"), isDeletion: isDeletion
        )
        return .init(id: targetId ?? draft.id, resource: nil, draft: draft, inherited: false)
    }
}
