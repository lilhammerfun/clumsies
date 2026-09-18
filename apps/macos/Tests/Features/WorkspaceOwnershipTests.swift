import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class WorkspaceOwnershipTests: XCTestCase {
    func testFailedSavePreservesEditsAndBlocksProjectSwitchAndSignOut() async {
        let workspace = WorkspaceCoordinator(storeDraft: { _ in throw SaveFailure.rejected })
        let item = prepare(workspace)
        var document = item.document
        document.body = "Unsaved editor text"
        workspace.edits.stageDocumentSave(item, document: document)
        defer { workspace.clearAuthorityScopedWorkspace() }

        await workspace.selectProject("second")
        XCTAssertEqual(workspace.context.activeProjectId, "first")
        XCTAssertFalse(workspace.context.isSwitchingMemoryContext)
        XCTAssertNil(workspace.context.loadingProjectId)

        await workspace.showOrgMemory()
        XCTAssertEqual(workspace.context.activeProjectId, "first")
        await workspace.signOut()
        XCTAssertEqual(workspace.context.phase, .ready)
        XCTAssertEqual(workspace.context.activeProjectId, "first")
        XCTAssertEqual(workspace.edits.pendingDocument(for: item), document)
        XCTAssertTrue(workspace.hasPendingChanges)
        XCTAssertNotNil(workspace.feedback.errorMessage)
    }

    func testAuthorityResetCancelsPendingSavesAndDocumentWork() async throws {
        let writes = WriteCounter()
        let workspace = WorkspaceCoordinator(storeDraft: { _ in
            await writes.record()
            throw SaveFailure.rejected
        })
        let item = prepare(workspace)
        workspace.edits.stageDocumentSave(item, document: item.document)
        let key = MemoryDocumentSessionKey(projectId: "first", itemId: item.id)
        let task = Task<Void, Never> { try? await Task.sleep(for: .seconds(10)) }
        workspace.sessions.documentSynchronizationTasks[key] = task
        workspace.sessions.synchronizingDocumentSessions.insert(key)
        let generation = workspace.context.workspaceReloadGeneration

        workspace.clearAuthorityScopedWorkspace()

        XCTAssertTrue(task.isCancelled)
        XCTAssertTrue(workspace.sessions.synchronizingDocumentSessions.isEmpty)
        XCTAssertTrue(workspace.catalog.resources.isEmpty)
        XCTAssertFalse(workspace.hasPendingChanges)
        XCTAssertNotEqual(workspace.context.workspaceReloadGeneration, generation)
        try await Task.sleep(for: .milliseconds(700))
        let count = await writes.count
        XCTAssertEqual(count, 0)
    }

    func testViewsObserveOwnersUsedByDerivedFeatureState() {
        let workspace = WorkspaceCoordinator()
        _ = prepare(workspace)
        workspace.bundles.bundles = [bundle(name: "Before")]
        let recorder = ProjectionRecorder()
        let host = NSHostingView(rootView: ProjectionProbe(recorder: recorder).workspaceEnvironment(workspace))
        host.frame = NSRect(x: 0, y: 0, width: 400, height: 100)
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(recorder.value, "First|Before")

        workspace.catalog.resources[0].document.title = "Updated"
        workspace.bundles.bundles = [bundle(name: "After")]
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(recorder.value, "Updated|After")
        workspace.catalog.resources.removeAll()
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(recorder.value, "|After")
    }

    func testWorkspaceOwnershipDoesNotRetainTheCompositionAfterRelease() {
        weak var context: WorkspaceContext?
        weak var navigation: WorkspaceNavigation?
        weak var memory: MemoryModel?
        weak var bundles: BundleStore?
        do {
            let workspace = WorkspaceCoordinator()
            context = workspace.context
            navigation = workspace.navigation
            memory = workspace.memory
            bundles = workspace.bundles
        }
        XCTAssertNil(context)
        XCTAssertNil(navigation)
        XCTAssertNil(memory)
        XCTAssertNil(bundles)
    }

    func testAnOldDraftWriteCannotRepopulateTheWorkspaceAfterAuthorityReset() async throws {
        let write = PausedDraftWrite()
        let workspace = WorkspaceCoordinator(storeDraft: { _ in await write.run() })
        let item = prepare(workspace)
        let save = Task { try await workspace.edits.save(item, document: item.document) }
        await write.waitUntilStarted()
        workspace.clearAuthorityScopedWorkspace()
        await write.finish()
        do {
            try await save.value
            XCTFail("An old authority write must be rejected before refreshing its draft")
        } catch is CancellationError {}
        XCTAssertTrue(workspace.edits.drafts.isEmpty)
        XCTAssertTrue(workspace.catalog.resources.isEmpty)
        XCTAssertNil(workspace.navigation.selectedItemId)
    }

    func testMissingBundleDeletionDoesNotChangeSelection() async {
        let workspace = WorkspaceCoordinator()
        workspace.bundles.bundles = [bundle(name: "First", id: "first"), bundle(name: "Second", id: "second")]
        workspace.bundleSelection.selectedBundleId = "second"

        await workspace.bundleSelection.deleteBundle(bundle(name: "Missing", id: "missing"))

        XCTAssertEqual(workspace.bundleSelection.selectedBundleId, "second")
        XCTAssertEqual(workspace.bundles.bundles.map(\.id), ["first", "second"])
    }

    @discardableResult
    private func prepare(_ workspace: WorkspaceCoordinator) -> MemoryListItem {
        workspace.context.phase = .ready
        workspace.context.projects = ["first", "second"].map {
            ProjectState(id: $0, name: $0, refCommitId: "commit", refEtag: "ref",
                selectedOrgResourceIds: ["memory"], orgSelectionRevision: 1, isLoaded: true)
        }
        workspace.context.activeProjectId = "first"
        let resource = MemoryResource(id: "memory", scope: .org, projectId: nil, projectName: nil,
            kind: .context, contentHash: "hash", updatedAt: "2026-09-18T00:00:00Z", refCommitId: "commit",
            contentLoaded: true, document: .init(title: "First", path: "guide.md", body: "Text"))
        workspace.catalog.resources = [resource]
        return MemoryListItem(id: resource.id, resource: resource, draft: nil, inherited: true,
            projectContextId: "first")
    }

    private func bundle(name: String, id: String = "bundle") -> PersonalBundle {
        PersonalBundle(id: id, name: name, description: "", resourceIds: [], revision: 1,
            updatedAt: "2026-09-18T00:00:00Z")
    }
}

private enum SaveFailure: Error { case rejected }

private actor WriteCounter {
    private(set) var count = 0
    func record() { count += 1 }
}

private actor PausedDraftWrite {
    private var started = false
    private var starts: [CheckedContinuation<Void, Never>] = []
    private var completion: CheckedContinuation<DaemonDraftOperationResponse, Never>?

    func run() async -> DaemonDraftOperationResponse {
        started = true
        starts.forEach { $0.resume() }
        starts.removeAll()
        return await withCheckedContinuation { completion = $0 }
    }

    func waitUntilStarted() async {
        if started { return }
        await withCheckedContinuation { starts.append($0) }
    }

    func finish() {
        completion?.resume(returning: .init(localOperationId: "operation", draftId: "old-draft", queued: true, syncStatus: .queued))
        completion = nil
    }
}

@MainActor
private final class ProjectionRecorder {
    var value = ""
}

private struct ProjectionProbe: View {
    @EnvironmentObject private var catalog: MemoryCatalog
    @EnvironmentObject private var drafts: DraftStore
    @EnvironmentObject private var context: WorkspaceContext
    @EnvironmentObject private var navigation: WorkspaceNavigation
    @EnvironmentObject private var memory: MemoryModel
    @EnvironmentObject private var bundleStore: BundleStore
    @EnvironmentObject private var bundles: BundlesModel
    let recorder: ProjectionRecorder

    var body: some View {
        let value = memory.visibleMemoryItems.map(\.document.title).joined(separator: ",")
            + "|" + (bundles.selectedBundle?.name ?? "")
        recorder.value = value
        return Text(value)
    }
}
