import XCTest
@testable import Clumsies

@MainActor
final class DocumentBaseLifetimeTests: XCTestCase {
    func testDirtyOrgEditorAndPendingSaveRetainOpenedCarrierAndSourceAcrossHeadAdvance() async throws {
        let writes = BaseWriteProbe()
        let workspace = WorkspaceCoordinator(storeDraft: { try await writes.write($0) })
        workspace.context.phase = .ready
        workspace.context.activeProjectId = "project"
        weak var carrier: CommitBasePin?
        weak var source: CommitBasePin?
        var editor: DocumentEditorModel? = {
            let projectPin = CommitBasePin(commitId: "project-old") { }
            let orgPin = CommitBasePin(commitId: "org-old") { }
            carrier = projectPin
            source = orgPin
            workspace.context.projects = [project("project-old", pin: projectPin)]
            let old = item("org-old", sourcePin: orgPin, editPin: projectPin)
            workspace.catalog.resources = [old.resource!]
            return model(old, in: workspace)
        }()
        editor?.document.body = "Unsaved body"
        let latest = item("org-new", path: "renamed.md",
            sourcePin: CommitBasePin(commitId: "org-new") { },
            editPin: CommitBasePin(commitId: "project-new") { })
        workspace.context.projects = [project("project-new", pin: latest.editBasePin)]
        workspace.catalog.resources = [latest.resource!]
        editor?.adoptAuthoritativeDocument(latest.document, item: latest)
        let document = try XCTUnwrap(editor?.document)
        XCTAssertEqual(document.body, "Unsaved body")
        XCTAssertEqual(document.path, "renamed.md")
        editor?.stageSave(document, item: latest, mode: .source)
        editor = nil
        XCTAssertNotNil(carrier)
        XCTAssertNotNil(source)
        let key = MemoryDocumentSessionKey(projectId: "project", itemId: "memory")
        XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.baseCommitId, "project-old")
        do { try await workspace.edits.flushDocumentSave(key); XCTFail("The rejected write must preserve edits.") }
        catch BaseWriteError.rejected { }
        let requests = await writes.requests
        XCTAssertEqual(requests.first?.baseCommitId, "project-old")
        XCTAssertEqual(requests.first?.baseCommitCaptured, true)
        XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.document.body, "Unsaved body")
        XCTAssertNotNil(carrier)
        XCTAssertNotNil(source)
        workspace.edits.cancelDocumentSave(key)
        await Task.yield()
        XCTAssertNil(carrier)
        XCTAssertNil(source)
        workspace.clearAuthorityScopedWorkspace()
    }

    func testExplicitEmptyBaseIsNotReplacedByNewProjectHead() async throws {
        let writes = BaseWriteProbe()
        let workspace = WorkspaceCoordinator(storeDraft: { try await writes.write($0) })
        workspace.context.phase = .ready
        workspace.context.activeProjectId = "project"
        workspace.context.projects = [project(nil)]
        let resource = MemoryResource(id: "memory", scope: .project, projectId: "project", projectName: "Project",
            kind: .context, contentHash: "hash", updatedAt: "now", refCommitId: nil,
            contentLoaded: true, document: .init(title: "Memory", path: "memory.md", body: "Body"))
        let original = MemoryListItem(id: "memory", resource: resource, draft: nil, inherited: false,
            projectContextId: "project")
        let editor = model(original, in: workspace)
        workspace.context.projects = [project("new-head", pin: CommitBasePin(commitId: "new-head") { })]
        editor.document.body = "Unsaved body"
        editor.stageSave(editor.document, item: original, mode: .source)
        do { try await workspace.edits.flushDocumentSave(original); XCTFail("Expected a controlled rejection.") }
        catch BaseWriteError.rejected { }
        let requests = await writes.requests
        XCTAssertEqual(requests.count, 1)
        XCTAssertNil(requests.first?.baseCommitId)
        XCTAssertEqual(requests.first?.baseCommitCaptured, true)
        let data = try JSONCoding.encoder().encode(try XCTUnwrap(requests.first))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(object["base_commit_captured"] as? Bool, true)
        workspace.clearAuthorityScopedWorkspace()
    }

    func testCleanAuthoritativeAdoptionReleasesOldBaseAndReconciliationRetainsItsCopies() {
        let workspace = WorkspaceCoordinator()
        workspace.context.phase = .ready
        workspace.context.activeProjectId = "project"
        weak var old: CommitBasePin?
        var editor: DocumentEditorModel? = {
            let pin = CommitBasePin(commitId: "old") { }
            old = pin
            workspace.context.projects = [project("old", pin: pin)]
            let original = item("old", sourcePin: pin, editPin: pin)
            let key = MemoryDocumentSessionKey(projectId: "project", itemId: "memory")
            workspace.sessions.documentBasePins[key] = [pin]
            return model(original, in: workspace)
        }()
        let latest = item("new", sourcePin: CommitBasePin(commitId: "new") { },
            editPin: CommitBasePin(commitId: "new") { })
        workspace.context.projects = [project("new", pin: latest.editBasePin)]
        editor?.adoptAuthoritativeDocument(latest.document, item: latest)
        editor = nil
        XCTAssertNotNil(old, "Closing an editor does not finish its separate reconciliation window.")
        workspace.sessions.finishDocumentReconciliation(for: .init(projectId: "project", itemId: "memory"))
        XCTAssertNil(old)
        workspace.clearAuthorityScopedWorkspace()
    }

    func testRemoteTerminalMissingOrDeletionDraftDoesNotDropPendingTextOrResurrectDraft() async throws {
        for state in ["missing", "merged", "discarded", "deleted"] {
            let writes = BaseWriteProbe()
            let workspace = WorkspaceCoordinator(storeDraft: { try await writes.write($0) })
            workspace.context.phase = .ready
            workspace.context.activeProjectId = "project"
            weak var retainedBase: CommitBasePin?
            var opened: MemoryListItem? = {
                let pin = CommitBasePin(commitId: "old") { }
                retainedBase = pin
                let draft = LocalDraft(id: "draft", projectId: "project", serverId: nil, serverVersion: 1,
                    baseCommitId: "old", currentCommitId: "old", freshness: .current,
                    hasUpstreamResourceChanges: false, reconciliation: .unknown, reconciliationCandidateId: nil,
                    scope: .project, kind: .context, targetId: nil, status: .open, origin: .desktop,
                    syncStatus: .synced, updatedAt: "now",
                    document: .init(title: "Draft", path: "draft.md", body: "Saved"), isDeletion: false,
                    baseCommitPin: pin)
                workspace.edits.drafts = [draft]
                return .init(id: "draft", resource: nil, draft: draft, inherited: false, projectContextId: "project")
            }()
            var dirty = try XCTUnwrap(opened?.document)
            dirty.body = "Unsaved body"
            workspace.edits.stageDocumentSave(try XCTUnwrap(opened), document: dirty)
            if state == "missing" { workspace.edits.drafts = [] }
            else {
                let original = try XCTUnwrap(opened?.draft)
                workspace.edits.drafts = [LocalDraft(id: original.id, projectId: original.projectId,
                    serverId: nil, serverVersion: 2, baseCommitId: "old", currentCommitId: "new",
                    freshness: .behind, hasUpstreamResourceChanges: true, reconciliation: .unknown,
                    reconciliationCandidateId: nil, scope: .project, kind: .context, targetId: nil,
                    status: state == "merged" ? .merged : state == "discarded" ? .discarded : .open,
                    origin: .desktop, syncStatus: .synced, updatedAt: "later", document: original.document,
                    isDeletion: state == "deleted")]
            }
            opened = nil
            let key = MemoryDocumentSessionKey(projectId: "project", itemId: "draft")
            do { try await workspace.edits.flushDocumentSave(key); XCTFail("Remote state changed.") }
            catch let error as DocumentSyncError { XCTAssertEqual(error, .checkoutNoLongerCurrent) }
            let requests = await writes.requests
            XCTAssertTrue(requests.isEmpty, "A terminated Draft must not be recreated by debounce.")
            XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.document.body, "Unsaved body")
            XCTAssertNotNil(retainedBase)
            workspace.edits.cancelDocumentSave(key)
            await Task.yield()
            XCTAssertNil(retainedBase)
            workspace.clearAuthorityScopedWorkspace()
        }
    }

    func testSameDraftRebasedBeforeFlushStillUsesCapturedBaseAndKeepsRejectedText() async throws {
        let writes = BaseWriteProbe()
        let workspace = WorkspaceCoordinator(storeDraft: { try await writes.write($0) })
        workspace.context.phase = .ready
        workspace.context.activeProjectId = "project"
        func draft(_ base: String, pin: CommitBasePin? = nil) -> LocalDraft {
            .init(id: "draft", projectId: "project", serverId: "server-draft", serverVersion: 2,
                baseCommitId: base, currentCommitId: base, freshness: .current,
                hasUpstreamResourceChanges: false, reconciliation: .unknown, reconciliationCandidateId: nil,
                scope: .project, kind: .context, targetId: nil, status: .open, origin: .desktop,
                syncStatus: .synced, updatedAt: "now",
                document: .init(title: "Draft", path: "draft.md", body: "Saved"), isDeletion: false,
                baseCommitPin: pin)
        }
        weak var retainedBase: CommitBasePin?
        var opened: MemoryListItem? = {
            let pin = CommitBasePin(commitId: "opened-base") { }
            retainedBase = pin
            let original = draft("opened-base", pin: pin)
            workspace.edits.drafts = [original]
            return .init(id: "draft", resource: nil, draft: original, inherited: false, projectContextId: "project")
        }()
        var document = try XCTUnwrap(opened?.document)
        document.body = "Unsaved before remote rebase"
        workspace.edits.stageDocumentSave(try XCTUnwrap(opened), document: document,
            capturedBaseCommitId: .some("opened-base"))
        opened = nil
        workspace.edits.drafts = [draft("rebased-base", pin: CommitBasePin(commitId: "rebased-base") { })]
        let key = MemoryDocumentSessionKey(projectId: "project", itemId: "draft")
        do { try await workspace.edits.flushDocumentSave(key); XCTFail("Backend must reject the stale captured Base.") }
        catch BaseWriteError.rejected { }
        let requests = await writes.requests
        XCTAssertEqual(requests.count, 1)
        XCTAssertEqual(requests.first?.draftId, "draft")
        XCTAssertEqual(requests.first?.baseCommitId, "opened-base")
        XCTAssertEqual(requests.first?.baseCommitCaptured, true)
        XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.document.body, document.body)
        XCTAssertNotNil(retainedBase)
        workspace.edits.cancelDocumentSave(key)
        await Task.yield()
        XCTAssertNil(retainedBase)
        workspace.clearAuthorityScopedWorkspace()
    }

    func testFirstOpenUsesProjectResourceBaseAndOrgCarrierWithoutCoalescingEmptyBase() async throws {
        for scope in [MemoryScope.project, .org] {
            for resourceBase in [String?.some("resource-A"), nil] {
                let writes = BaseWriteProbe()
                let workspace = WorkspaceCoordinator(storeDraft: { try await writes.write($0) })
                defer { workspace.clearAuthorityScopedWorkspace() }
                let carrier = CommitBasePin(commitId: "carrier-B") { }
                workspace.context.phase = .ready
                workspace.context.activeProjectId = "project"
                workspace.context.projects = [project("carrier-B", pin: carrier)]
                let resource = MemoryResource(id: "memory", scope: scope,
                    projectId: scope == .project ? "project" : nil, projectName: "Project", kind: .context,
                    contentHash: "hash", updatedAt: "now", refCommitId: resourceBase, contentLoaded: true,
                    document: .init(title: "Memory", path: "memory.md", body: "Old snapshot body"),
                    baseCommitPin: resourceBase.map { CommitBasePin(commitId: $0) { } })
                workspace.catalog.resources = [resource]
                let opened = try XCTUnwrap(MemoryTreeProjection.items(
                    resources: [resource], drafts: [], activeProjectId: "project",
                    selectedOrgResourceIds: [resource.id], editBasePin: carrier
                ).first)
                XCTAssertEqual(opened.editBasePin?.commitId, "carrier-B")
                let expectedBase = scope == .project ? resourceBase : "carrier-B"
                let editor = model(opened, in: workspace)
                editor.document.body = "Edited old snapshot"
                editor.stageSave(editor.document, item: opened, mode: .source)
                do { try await workspace.edits.flushDocumentSave(opened); XCTFail("Expected controlled rejection.") }
                catch BaseWriteError.rejected { }
                // The uncaptured store entry points must follow the same Base
                // ownership when used by non-editor callers.
                workspace.edits.stageDocumentSave(opened, document: editor.document)
                do { try await workspace.edits.flushDocumentSave(opened); XCTFail("Expected controlled rejection.") }
                catch BaseWriteError.rejected { }
                do { try await workspace.edits.save(opened, document: editor.document); XCTFail("Expected controlled rejection.") }
                catch BaseWriteError.rejected { }
                let requests = await writes.requests
                XCTAssertEqual(requests.count, 3)
                XCTAssertTrue(requests.allSatisfy { $0.baseCommitId == expectedBase && $0.baseCommitCaptured })
                let key = MemoryDocumentSessionKey(projectId: "project", itemId: "memory")
                XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.baseCommitId, expectedBase)
                XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.document.body, editor.document.body)
            }
        }
    }

    private func project(_ commitId: String?, pin: CommitBasePin? = nil) -> ProjectState {
        .init(id: "project", name: "Project", refCommitId: commitId, refEtag: "ref",
            selectedOrgResourceIds: ["memory"], orgSelectionRevision: 1, isLoaded: true, baseCommitPin: pin)
    }

    private func item(_ commitId: String, path: String = "memory.md", sourcePin: CommitBasePin,
                      editPin: CommitBasePin) -> MemoryListItem {
        let resource = MemoryResource(id: "memory", scope: .org, projectId: nil, projectName: nil,
            kind: .context, contentHash: "hash", updatedAt: "now", refCommitId: commitId,
            contentLoaded: true, document: .init(title: "Memory", path: path, body: "Body"), baseCommitPin: sourcePin)
        return .init(id: "memory", resource: resource, draft: nil, inherited: true,
            projectContextId: "project", editBasePin: editPin)
    }

    private func model(_ item: MemoryListItem, in workspace: WorkspaceCoordinator) -> DocumentEditorModel {
        .init(item: item, drafts: workspace.edits, context: workspace.context, feedback: workspace.feedback,
            sessions: workspace.sessions, memory: workspace.memory, reviews: workspace.reviews,
            reconciliation: workspace.reconciliation)
    }
}

private enum BaseWriteError: Error { case rejected }

private actor BaseWriteProbe {
    private(set) var requests: [DaemonDraftOperationRequest] = []
    func write(_ request: DaemonDraftOperationRequest) throws -> DaemonDraftOperationResponse {
        requests.append(request)
        throw BaseWriteError.rejected
    }
}
