import XCTest
@testable import Clumsies

final class MemoryFileTreeMenuTests: XCTestCase {
    private func resourceItem(
        _ id: String,
        scope: MemoryScope,
        inherited: Bool,
        projectId: String? = nil,
        draft: LocalDraft? = nil,
        path: String? = nil,
        kind: MemoryKind = .context
    ) -> MemoryListItem {
        MemoryListItem(
            id: id,
            resource: MemoryResource(
                id: id,
                scope: scope,
                projectId: projectId,
                projectName: nil,
                kind: kind,
                contentHash: "hash-\(id)",
                updatedAt: "2026-01-01T00:00:00Z",
                refCommitId: nil,
                contentLoaded: false,
                document: EditableMemoryDocument(
                    title: id,
                    path: path ?? "\(id).md",
                    body: ""
                )
            ),
            draft: draft,
            inherited: inherited
        )
    }

    private func localDraft(
        _ id: String,
        scope: MemoryScope,
        targetId: String? = nil,
        isDeletion: Bool = false,
        status: DaemonLocalDraftStatus = .open,
        serverId: String? = nil,
        syncStatus: DaemonDraftSyncState = .queued,
        freshness: DraftFreshness = .current,
        reconciliation: DraftReconciliationStatus = .clean,
        path: String? = nil,
        kind: MemoryKind = .context
    ) -> LocalDraft {
        LocalDraft(
            id: id,
            projectId: "p1",
            serverId: serverId,
            serverVersion: 0,
            baseCommitId: nil,
            currentCommitId: nil,
            freshness: freshness,
            hasUpstreamResourceChanges: false,
            reconciliation: reconciliation,
            reconciliationCandidateId: nil,
            scope: scope,
            kind: kind,
            targetId: targetId,
            status: status,
            origin: .desktop,
            syncStatus: syncStatus,
            updatedAt: "2026-01-01T00:00:00Z",
            document: EditableMemoryDocument(
                title: id,
                path: path ?? "\(id).md",
                body: ""
            ),
            isDeletion: isDeletion
        )
    }

    private func draftItem(
        _ id: String,
        scope: MemoryScope,
        targetId: String? = nil,
        isDeletion: Bool = false,
        path: String? = nil,
        kind: MemoryKind = .context
    ) -> MemoryListItem {
        MemoryListItem(
            id: targetId ?? id,
            resource: nil,
            draft: localDraft(
                id,
                scope: scope,
                targetId: targetId,
                isDeletion: isDeletion,
                path: path,
                kind: kind
            ),
            inherited: false
        )
    }

    func testMemoryExportLoadsBodiesAndPreservesDraftAndPendingEdits() async throws {
        var renamedDraft = localDraft("draft", scope: .org, targetId: "renamed", path: "new/name.md")
        renamedDraft.document.body = "Draft body\n"
        let items = [
            resourceItem("unloaded", scope: .org, inherited: true),
            resourceItem("renamed", scope: .org, inherited: true, draft: renamedDraft),
            draftItem("created", scope: .org, path: "new/empty.md"),
            draftItem("deleted", scope: .org, isDeletion: true),
            resourceItem("pending", scope: .org, inherited: true),
        ]
        let pending = EditableMemoryDocument(title: "Pending", path: "pending.md", body: "Latest editor text")
        let documents = try await MemoryModel.memoryExportDocuments(
            items, pendingDocuments: ["pending": pending]
        ) { resource in
            XCTAssertEqual(resource.id, "unloaded")
            var loaded = resource
            loaded.contentLoaded = true
            loaded.document.body = "Loaded body\n"
            return loaded
        }
        XCTAssertEqual(documents.map(\.path), ["unloaded.md", "new/name.md", "new/empty.md", "pending.md"])
        XCTAssertEqual(documents.map(\.body), ["Loaded body\n", "Draft body\n", "", pending.body])
    }

    func testMemoryExportFailsOnUnavailableBodiesInsteadOfWritingPlaceholders() async {
        var draft = localDraft("orphan", scope: .org, targetId: "missing")
        draft.documentBaselineAvailable = false
        let orphan = MemoryListItem(id: "missing", resource: nil, draft: draft, inherited: false)
        for items in [[orphan], [resourceItem("unloaded", scope: .org, inherited: true)]] {
            do {
                _ = try await MemoryModel.memoryExportDocuments(items) { _ in
                    throw CocoaError(.fileReadNoPermission)
                }
                XCTFail("Export must fail when a complete body cannot be read")
            } catch {
                XCTAssertFalse(error.localizedDescription.isEmpty)
            }
        }
    }

    // MARK: - Org view

    func testOrgViewAddableIncludesPublishedOrgItems() {
        let items = [
            resourceItem("org-a", scope: .org, inherited: false),
            resourceItem("org-b", scope: .org, inherited: false),
            draftItem("draft-a", scope: .org),
        ]
        XCTAssertEqual(MemoryFileTreeMenu.addable(items, inOrgView: true).map(\.id), ["org-a", "org-b"])
    }

    func testOrgViewHasNoRemovableOrInheritedItems() {
        let items = [
            resourceItem("org-a", scope: .org, inherited: false),
            resourceItem("org-b", scope: .org, inherited: true),
        ]
        XCTAssertTrue(MemoryFileTreeMenu.removable(items, inOrgView: true).isEmpty)
    }

    func testOrgViewIsReadOnlyForAuthorityMutations() {
        let items = [
            resourceItem("org-a", scope: .org, inherited: false),
            draftItem("draft-a", scope: .org),
        ]
        XCTAssertTrue(MemoryFileTreeMenu.trashable(items, inOrgView: true).isEmpty)
        XCTAssertFalse(MemoryFileTreeMenu.canRename(items[0], inOrgView: true))
        XCTAssertFalse(
            MemoryFileTreeMenu.canProposeOrganizationDeletion(items[0], inOrgView: true)
        )
    }

    func testOrgViewDoesNotOfferNewMemoryCreation() {
        XCTAssertNil(MemoryFileTreeMenu.creationScope(inOrgView: true))
    }

    // MARK: - Project view

    func testProjectViewHasNoAddableItems() {
        let items = [
            resourceItem("org-a", scope: .org, inherited: false),
            resourceItem("own-a", scope: .project, inherited: false, projectId: "p1"),
        ]
        XCTAssertTrue(MemoryFileTreeMenu.addable(items, inOrgView: false).isEmpty)
    }

    func testProjectViewRemovableOnlyInheritedItems() {
        let items = [
            resourceItem("org-ref", scope: .org, inherited: true),
            resourceItem("org-unref", scope: .org, inherited: false),
            resourceItem("own-a", scope: .project, inherited: false, projectId: "p1"),
        ]
        XCTAssertEqual(MemoryFileTreeMenu.removable(items, inOrgView: false).map(\.id), ["org-ref"])
    }

    func testProjectViewAuthorityActionsOnlyIncludeSelectedOrgResources() {
        let items = [
            resourceItem("org-ref", scope: .org, inherited: true),
            resourceItem("org-unref", scope: .org, inherited: false),
            resourceItem("own-a", scope: .project, inherited: false, projectId: "p1"),
        ]
        XCTAssertEqual(
            items.map { MemoryFileTreeMenu.canRename($0, inOrgView: false) },
            [true, false, false]
        )
        XCTAssertEqual(
            MemoryFileTreeMenu.trashable(items, inOrgView: false).map(\.id),
            ["org-ref"]
        )
    }

    func testProjectViewCreatesProjectBoundOrgProposals() {
        XCTAssertEqual(MemoryFileTreeMenu.creationScope(inOrgView: false), .org)
    }

    func testProjectViewCanRenamePureCreateDraft() {
        let item = draftItem("draft", scope: .org, path: "notes/new.md")

        XCTAssertTrue(MemoryFileTreeMenu.canRename(item, inOrgView: false))
        XCTAssertFalse(MemoryFileTreeMenu.canRename(item, inOrgView: true))
    }

    func testDirectoryReviewIncludesOnlyOpenOrganizationDraftsInProjectView() {
        let openOrg = resourceItem(
            "org-open",
            scope: .org,
            inherited: true,
            draft: localDraft("draft-open", scope: .org, targetId: "org-open")
        )
        let submittedOrg = resourceItem(
            "org-submitted",
            scope: .org,
            inherited: true,
            draft: localDraft(
                "draft-submitted",
                scope: .org,
                targetId: "org-submitted",
                status: .submitted
            )
        )
        let project = draftItem("project-draft", scope: .project)

        XCTAssertEqual(
            MemoryFileTreeMenu.reviewableDrafts(
                [openOrg, submittedOrg, project],
                inOrgView: false
            ).map(\.id),
            ["draft-open"]
        )
        XCTAssertTrue(
            MemoryFileTreeMenu.reviewableDrafts([openOrg], inOrgView: true).isEmpty
        )
    }

    func testReviewSelectionAllowsBehindDraftsButRequiresEveryDraftToBeSynced() {
        let current = localDraft("current", scope: .org, serverId: "server-current", syncStatus: .synced)
        XCTAssertFalse(MemoryFileTreeMenu.isReviewSelectionReady([]))
        XCTAssertTrue(MemoryFileTreeMenu.isReviewSelectionReady([current]))

        for reconciliation: DraftReconciliationStatus in [.unknown, .clean, .conflicts] {
            let behind = localDraft(
                "behind", scope: .org, serverId: "server-behind", syncStatus: .synced,
                freshness: .behind, reconciliation: reconciliation
            )
            XCTAssertTrue(MemoryFileTreeMenu.isReviewSelectionReady([behind]))
            XCTAssertTrue(MemoryFileTreeMenu.isReviewSelectionReady([current, behind]))
        }

        for syncStatus: DaemonDraftSyncState in [.queued, .syncing, .retrying, .failed] {
            let unsynced = localDraft(
                "unsynced", scope: .org, serverId: "server-unsynced", syncStatus: syncStatus
            )
            XCTAssertFalse(MemoryFileTreeMenu.isReviewSelectionReady([current, unsynced]))
        }
        let missingServerId = localDraft("local", scope: .org, syncStatus: .synced)
        XCTAssertFalse(MemoryFileTreeMenu.isReviewSelectionReady([current, missingServerId]))
    }

    func testDirectoryDiscardIncludesEveryProjectCarriedDraftOnce() {
        let open = draftItem("open", scope: .org, path: "notes/open.md")
        let submitted = resourceItem(
            "shared",
            scope: .org,
            inherited: true,
            draft: localDraft(
                "submitted",
                scope: .org,
                targetId: "shared",
                status: .submitted,
                path: "notes/shared.md"
            ),
            path: "notes/shared.md"
        )

        XCTAssertEqual(
            MemoryFileTreeMenu.discardableDrafts(
                [open, submitted, submitted],
                inOrgView: false
            ).map(\.id),
            ["open", "submitted"]
        )
        XCTAssertTrue(
            MemoryFileTreeMenu.discardableDrafts([open], inOrgView: true).isEmpty
        )
    }

    func testDirectoryRenamePreservesNestedPathsAndWorkflowNamespace() throws {
        let context = resourceItem(
            "context",
            scope: .org,
            inherited: true,
            path: "guides/nested/context.md"
        )
        let workflow = resourceItem(
            "workflow",
            scope: .org,
            inherited: true,
            path: "workflow/guides/run.md",
            kind: .workflows
        )

        let plan = try MemoryFileTreeMenu.directoryRenamePlan(
            directoryId: "directory:guides",
            newName: "handbook",
            items: [context, workflow],
            occupiedPaths: [context.document.path, workflow.document.path],
            occupiedTreePaths: ["guides/nested/context.md", "guides/run.md"],
            inOrgView: false
        )

        XCTAssertEqual(
            Dictionary(uniqueKeysWithValues: plan.changes.map { ($0.item.id, $0.newPath) }),
            [
                "context": "handbook/nested/context.md",
                "workflow": "workflow/handbook/run.md",
            ]
        )
    }

    func testDirectoryRenameRejectsExistingDestinationDirectory() {
        let source = resourceItem(
            "source",
            scope: .org,
            inherited: true,
            path: "guides/source.md"
        )

        XCTAssertThrowsError(try MemoryFileTreeMenu.directoryRenamePlan(
            directoryId: "directory:guides",
            newName: "existing",
            items: [source],
            occupiedPaths: [source.document.path, "existing/other.md"],
            occupiedTreePaths: [source.document.path, "existing/other.md"],
            inOrgView: false
        )) { error in
            XCTAssertEqual(
                error as? MemoryDirectoryMutationError,
                .pathCollision("existing/source.md")
            )
        }
    }

    func testDirectoryRenameRejectsFileAtDestinationDirectory() {
        let source = resourceItem(
            "source",
            scope: .org,
            inherited: true,
            path: "guides/source.md"
        )

        XCTAssertThrowsError(try MemoryFileTreeMenu.directoryRenamePlan(
            directoryId: "directory:guides",
            newName: "handbook",
            items: [source],
            occupiedPaths: [source.document.path, "handbook"],
            occupiedTreePaths: [source.document.path, "handbook"],
            inOrgView: false
        )) { error in
            XCTAssertEqual(
                error as? MemoryDirectoryMutationError,
                .pathCollision("handbook/source.md")
            )
        }
    }

    func testDirectoryRenameRejectsFileAboveDestinationDirectory() {
        let source = resourceItem(
            "source",
            scope: .org,
            inherited: true,
            path: "parent/guides/source.md"
        )

        XCTAssertThrowsError(try MemoryFileTreeMenu.directoryRenamePlan(
            directoryId: "directory:parent/guides",
            newName: "handbook",
            items: [source],
            occupiedPaths: [source.document.path, "parent"],
            occupiedTreePaths: [source.document.path, "parent"],
            inOrgView: false
        )) { error in
            XCTAssertEqual(
                error as? MemoryDirectoryMutationError,
                .pathCollision("parent/handbook/source.md")
            )
        }
    }

    func testDirectoryRenameRejectsCaseInsensitiveDestinationCollision() {
        let source = resourceItem(
            "source",
            scope: .org,
            inherited: true,
            path: "guides/source.md"
        )

        XCTAssertThrowsError(try MemoryFileTreeMenu.directoryRenamePlan(
            directoryId: "directory:guides",
            newName: "handbook",
            items: [source],
            occupiedPaths: [source.document.path, "HandBook/other.md"],
            occupiedTreePaths: [source.document.path, "HandBook/other.md"],
            inOrgView: false
        )) { error in
            XCTAssertEqual(
                error as? MemoryDirectoryMutationError,
                .pathCollision("handbook/source.md")
            )
        }
    }

    func testDirectoryRenameRejectsHiddenWorkflowTreeCollision() {
        let source = resourceItem(
            "source",
            scope: .org,
            inherited: true,
            path: "guides/source.md"
        )

        XCTAssertThrowsError(try MemoryFileTreeMenu.directoryRenamePlan(
            directoryId: "directory:guides",
            newName: "handbook",
            items: [source],
            occupiedPaths: [source.document.path, "workflow/handbook/other.md"],
            occupiedTreePaths: [source.document.path, "handbook/other.md"],
            inOrgView: false
        )) { error in
            XCTAssertEqual(
                error as? MemoryDirectoryMutationError,
                .pathCollision("handbook/source.md")
            )
        }
    }

    func testDirectoryDeletionCombinesSharedDeletionAndPureDraftDiscard() throws {
        let shared = resourceItem(
            "shared",
            scope: .org,
            inherited: true,
            path: "notes/shared.md"
        )
        let draft = draftItem("draft", scope: .org, path: "notes/new.md")
        let plan = try XCTUnwrap(
            MemoryFileTreeMenu.directoryDeletionPlan(
                [shared, draft],
                inOrgView: false
            )
        )

        XCTAssertEqual(plan.itemsToDelete.map(\.id), ["shared"])
        XCTAssertEqual(plan.draftsToDiscard.map(\.id), ["draft"])
    }

    func testDirectoryMutationRejectsLegacyProjectAuthority() {
        let legacy = resourceItem(
            "legacy",
            scope: .project,
            inherited: false,
            projectId: "p1",
            path: "notes/legacy.md"
        )

        XCTAssertNil(
            MemoryFileTreeMenu.directoryDeletionPlan([legacy], inOrgView: false)
        )
        XCTAssertThrowsError(try MemoryFileTreeMenu.directoryRenamePlan(
            directoryId: "directory:notes",
            newName: "renamed",
            items: [legacy],
            occupiedPaths: [legacy.document.path],
            occupiedTreePaths: [legacy.document.path],
            inOrgView: false
        )) { error in
            XCTAssertEqual(error as? MemoryDirectoryMutationError, .readOnly)
        }
    }

    func testSelectedOrgAuthorityOffersExplicitMutationActionsInProjectView() {
        let item = resourceItem("org-ref", scope: .org, inherited: true)

        XCTAssertTrue(MemoryFileTreeMenu.canRename(item, inOrgView: false))
        XCTAssertTrue(MemoryFileTreeMenu.canProposeOrganizationDeletion(item, inOrgView: false))
        XCTAssertEqual(MemoryFileTreeMenu.trashable([item], inOrgView: false), [item])
    }

    func testUnselectedOrgAuthorityDoesNotOfferMutationActionsInProjectView() {
        let item = resourceItem("org-unselected", scope: .org, inherited: false)

        XCTAssertFalse(MemoryFileTreeMenu.canRename(item, inOrgView: false))
        XCTAssertFalse(MemoryFileTreeMenu.canProposeOrganizationDeletion(item, inOrgView: false))
        XCTAssertTrue(MemoryFileTreeMenu.trashable([item], inOrgView: false).isEmpty)
    }

    func testTargetBackedDraftOnlyRowDoesNotOfferAuthorityMutationActions() {
        let item = draftItem("draft", scope: .org, targetId: "org-removed")

        XCTAssertFalse(MemoryFileTreeMenu.canRename(item, inOrgView: false))
        XCTAssertFalse(MemoryFileTreeMenu.canProposeOrganizationDeletion(item, inOrgView: false))
        XCTAssertTrue(MemoryFileTreeMenu.trashable([item], inOrgView: false).isEmpty)
    }

    func testDeletionDraftDoesNotOfferDeletionAgain() {
        let item = resourceItem(
            "org-ref",
            scope: .org,
            inherited: true,
            draft: localDraft(
                "draft-delete",
                scope: .org,
                targetId: "org-ref",
                isDeletion: true
            )
        )

        XCTAssertFalse(MemoryFileTreeMenu.canRename(item, inOrgView: false))
        XCTAssertFalse(MemoryFileTreeMenu.canProposeOrganizationDeletion(item, inOrgView: false))
        XCTAssertTrue(MemoryFileTreeMenu.trashable([item], inOrgView: false).isEmpty)
    }

    func testLegacyProjectAuthorityDoesNotOfferOrganizationMutationActions() {
        let item = resourceItem(
            "legacy-project",
            scope: .project,
            inherited: false,
            projectId: "p1"
        )

        XCTAssertFalse(MemoryFileTreeMenu.canRename(item, inOrgView: false))
        XCTAssertFalse(MemoryFileTreeMenu.canProposeOrganizationDeletion(item, inOrgView: false))
        XCTAssertTrue(MemoryFileTreeMenu.trashable([item], inOrgView: false).isEmpty)
    }

    // MARK: - Mixed selection stays predictable

    func testMixedSelectionSplitsRemovableAndTrashable() {
        let items = [
            resourceItem("org-ref", scope: .org, inherited: true),
            resourceItem("own-a", scope: .project, inherited: false, projectId: "p1"),
            resourceItem("org-unref", scope: .org, inherited: false),
        ]
        XCTAssertEqual(MemoryFileTreeMenu.removable(items, inOrgView: false).map(\.id), ["org-ref"])
        XCTAssertEqual(
            MemoryFileTreeMenu.trashable(items, inOrgView: false).map(\.id),
            ["org-ref"]
        )
    }

    // MARK: - Destructive operation confirmation

    func testDestructiveAlertsDescribeTheirActualEffects() {
        let shared = resourceItem(
            "shared",
            scope: .org,
            inherited: true,
            path: "notes/shared.md"
        )
        let draft = localDraft(
            "draft",
            scope: .org,
            path: "notes/new.md"
        )

        let organization = MemoryFileTreeAlert.organizationDeletion(items: [shared])
        XCTAssertEqual(organization.title, "Delete File?")
        XCTAssertEqual(organization.confirmationTitle, "Delete")
        XCTAssertTrue(organization.message.contains("every project"))

        let discard = MemoryFileTreeAlert.directoryDiscard(
            name: "notes",
            drafts: [draft]
        )
        XCTAssertEqual(discard.title, "Discard Draft in notes?")
        XCTAssertEqual(discard.confirmationTitle, "Discard Drafts")
        XCTAssertTrue(discard.message.contains("Remote Organization Memory is unchanged"))

        let deletion = MemoryFileTreeAlert.directoryDeletion(
            name: "notes",
            plan: .init(itemsToDelete: [shared], draftsToDiscard: [draft])
        )
        XCTAssertEqual(deletion.title, "Delete notes?")
        XCTAssertEqual(deletion.confirmationTitle, "Delete Folder")
        XCTAssertTrue(deletion.message.contains("create deletion proposals"))
        XCTAssertTrue(deletion.message.contains("discard 1 unpublished draft"))
    }

    func testFileTreeOwnsExactlyOneAlertPresenter() throws {
        let macOSRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/Memory/MemoryFileTreeView.swift"),
            encoding: .utf8
        )
        let start = try XCTUnwrap(source.range(of: "struct FileTreeView: View {"))
        let end = try XCTUnwrap(
            source[start.lowerBound...].range(of: "\nprivate struct FileTreeRow: View")
        )
        let fileTree = source[start.lowerBound..<end.lowerBound]

        XCTAssertEqual(fileTree.components(separatedBy: ".alert(").count - 1, 1)
        XCTAssertTrue(fileTree.contains("pendingAlert = .organizationDeletion"))
        XCTAssertTrue(fileTree.contains("pendingAlert = .directoryDiscard"))
        XCTAssertTrue(fileTree.contains("pendingAlert = .directoryDeletion"))
    }

    func testSingleDraftDiscardIsBlockedDuringDirectoryOperation() throws {
        let macOSRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/Memory/MemoryFileTreeView.swift"),
            encoding: .utf8
        ).replacingOccurrences(of: "self.", with: "")
        let start = try XCTUnwrap(
            source.range(of: "if !isOrgView, let draft = singleItem.draft {")
        )
        let end = try XCTUnwrap(
            source[start.lowerBound...].range(of: "\n        if targetItems.isEmpty {")
        )
        let discardAction = source[start.lowerBound..<end.lowerBound]

        XCTAssertTrue(
            discardAction.contains("guard operations.directoryOperationProgress == nil else { return }")
        )
        XCTAssertTrue(discardAction.contains("operations.directoryOperationProgress != nil"))
        XCTAssertTrue(discardAction.contains("|| singleSynchronizing"))
    }

    func testItemMutationsAreBlockedDuringDirectoryOperation() throws {
        let macOSRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/Memory/MemoryFileTreeView.swift"),
            encoding: .utf8
        ).replacingOccurrences(of: "self.", with: "")

        XCTAssertGreaterThanOrEqual(
            source.components(
                separatedBy: ".disabled(operations.directoryOperationProgress != nil || singleSynchronizing)"
            ).count - 1,
            2
        )
        XCTAssertTrue(source.contains(
            "operations.directoryOperationProgress != nil\n                        "
                + "|| trashSelectionContainsSynchronizingDocument"
        ))
        XCTAssertTrue(source.contains(
            "operations.directoryOperationProgress != nil\n                    "
                + "|| !reviewSelectionIsReady"
        ))
        XCTAssertTrue(source.contains(
            "guard operations.directoryOperationProgress == nil else { return }\n"
                + "        guard let item = itemToRename"
        ))
    }
}
