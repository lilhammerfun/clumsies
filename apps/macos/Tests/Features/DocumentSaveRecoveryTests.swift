import AppKit
import Combine
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class DocumentSaveRecoveryTests: XCTestCase {
    func testReopeningFailedSaveRestoresOriginalItemBaseAndPinsAfterHeadOrDraftAdvance() async throws {
        for scenario in ["published", "draft", "empty"] {
            let writes = RecoveryWriteProbe()
            let workspace = WorkspaceCoordinator(storeDraft: { try await writes.write($0) })
            workspace.context.phase = .ready
            workspace.context.activeProjectId = "project"
            let base: String? = scenario == "empty" ? nil : "opened-base"
            weak var retained: CommitBasePin?
            var editor: DocumentEditorModel? = {
                let pin = base.map { CommitBasePin(commitId: $0) { } }
                retained = pin
                workspace.context.projects = [project(base, pin: pin)]
                let opened = scenario == "published" ? published(base, pin: pin) : draftItem(base, pin: pin)
                workspace.catalog.resources = opened.resource.map { [$0] } ?? []
                workspace.edits.drafts = opened.draft.map { [$0] } ?? []
                let editor = model(opened, in: workspace)
                editor.document.body = "Unsaved before close"
                editor.stageSave(editor.document, item: opened, mode: .source)
                return editor
            }()
            let itemId = scenario == "published" ? "memory" : "draft"
            let key = MemoryDocumentSessionKey(projectId: "project", itemId: itemId)
            do { try await workspace.edits.flushDocumentSave(key); XCTFail("Expected controlled rejection.") }
            catch RecoveryError.writeRejected { }
            editor = nil
            let nextPin = CommitBasePin(commitId: "advanced-base") { }
            workspace.context.projects = [project("advanced-base", pin: nextPin)]
            let latest = scenario == "published" ? published("advanced-base", pin: nextPin)
                : draftItem("advanced-base", pin: nextPin)
            workspace.catalog.resources = latest.resource.map { [$0] } ?? []
            workspace.edits.drafts = latest.draft.map { [$0] } ?? []
            editor = model(latest, in: workspace)
            XCTAssertEqual(editor?.document.body, "Unsaved before close")
            editor?.document.body += " and after reopen"
            editor?.stageSave(try XCTUnwrap(editor?.document), item: latest, mode: .source)
            let pending = try XCTUnwrap(workspace.edits.pendingDocumentSaves[key])
            XCTAssertEqual(pending.baseCommitId, base)
            XCTAssertEqual(pending.item.draft?.baseCommitId ?? pending.item.resource?.refCommitId, base)
            if base != nil { XCTAssertNotNil(retained) }
            do { try await workspace.edits.flushDocumentSave(key); XCTFail("Expected stale Base rejection.") }
            catch RecoveryError.writeRejected { }
            let requests = await writes.requests
            XCTAssertEqual(requests.count, 2)
            XCTAssertTrue(requests.allSatisfy { $0.baseCommitId == base && $0.baseCommitCaptured })
            XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.document.body,
                "Unsaved before close and after reopen")
            editor = nil
            workspace.clearAuthorityScopedWorkspace()
        }
    }

    func testFailedEditorDeleteAndDiscardKeepPendingTextAndPreventSuccessfulFlush() async throws {
        for action in ["delete", "discard", "missing-discard"] {
            let writes = RecoveryWriteProbe()
            let workspace = WorkspaceCoordinator(storeDraft: { try await writes.write($0) })
            workspace.context.phase = .ready
            workspace.context.activeProjectId = "project"
            let pin = CommitBasePin(commitId: "base") { }
            workspace.context.projects = [project("base", pin: pin)]
            let item = draftItem("base", pin: pin, targetId: "memory", resource: published("base", pin: pin).resource)
            workspace.edits.drafts = [try XCTUnwrap(item.draft)]
            let editor = model(item, in: workspace)
            editor.document.body = "Unsaved text to retain"
            editor.stageSave(editor.document, item: item, mode: .source)
            let completed = expectation(description: "Editor mutation finished")
            let observation = editor.$suppressesSaving.dropFirst().filter { !$0 }.sink { _ in completed.fulfill() }
            if action == "missing-discard" { workspace.edits.drafts = [] }
            if action == "delete" { editor.moveToTrash(item: item) }
            else { editor.discard(try XCTUnwrap(item.draft), item: item) }
            await fulfillment(of: [completed], timeout: 1)
            withExtendedLifetime(observation) { }
            let key = MemoryDocumentSessionKey(projectId: "project", itemId: "memory")
            XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.document.body, "Unsaved text to retain")
            XCTAssertTrue(workspace.edits.pendingDocumentSaves[key]?.item.draft?.baseCommitPin === pin)
            XCTAssertTrue(workspace.hasPendingChanges)
            let flushed = await workspace.flushPendingChanges()
            XCTAssertFalse(flushed, "A rejected save must still block exiting or switching.")
            XCTAssertEqual(editor.document.body, "Unsaved text to retain")
            workspace.clearAuthorityScopedWorkspace()
        }
    }

    func testDurableDeleteOrDiscardAcknowledgementClearsOnlyItsPendingSaveEvenWhenRefreshFails() async throws {
        for action in ["delete", "discard"] {
            let writes = RecoveryWriteProbe(pauses: true)
            let workspace = WorkspaceCoordinator()
            workspace.context.phase = .ready
            workspace.context.activeProjectId = "project"
            let pin = CommitBasePin(commitId: "base") { }
            workspace.context.projects = [project("base", pin: pin)]
            let item = draftItem("base", pin: pin, targetId: "memory", resource: published("base", pin: pin).resource)
            let edits = DraftStore(catalog: workspace.catalog, context: workspace.context,
                feedback: workspace.feedback, sessions: workspace.sessions,
                storeDraft: { try await writes.write($0) }, loadDraft: { _ in throw RecoveryError.refreshRejected })
            edits.drafts = [try XCTUnwrap(item.draft)]
            var text = item.document
            text.body = "Unsaved before destructive action"
            edits.stageDocumentSave(item, document: text)
            let key = MemoryDocumentSessionKey(projectId: "project", itemId: "memory")
            let draft = try XCTUnwrap(item.draft)
            let mutation = Task {
                if action == "delete" { return await edits.delete(item) }
                return await edits.discard(draft)
            }
            await writes.waitUntilStarted()
            XCTAssertEqual(edits.pendingDocumentSaves[key]?.document, text, "Keep text until a durable acknowledgement.")
            await writes.acknowledge()
            let succeeded = await mutation.value
            XCTAssertEqual(succeeded, action == "discard", "The controlled delete refresh fails after its write succeeded.")
            XCTAssertNil(edits.pendingDocumentSaves[key], "A refresh failure must not restore text discarded by a successful write.")
            try await edits.flushDocumentSave(key)
            let requests = await writes.requests
            XCTAssertEqual(requests.count, 1, "The successful destructive operation must not be resurrected by autosave.")
            edits.resetAuthority()
            workspace.clearAuthorityScopedWorkspace()
        }
    }

    func testNewEditsDuringSuccessfulExitFlushRemainPendingUntilTheirOwnAcknowledgement() async throws {
        for addsAnotherDocument in [false, true] {
            let writes = RecoveryWriteProbe(pauses: true)
            let workspace = WorkspaceCoordinator(storeDraft: { try await writes.write($0) },
                loadDraft: { try await writes.detail($0) })
            defer { workspace.clearAuthorityScopedWorkspace() }
            workspace.context.phase = .ready
            workspace.context.activeProjectId = "project"
            let pin = CommitBasePin(commitId: "base") { }
            workspace.context.projects = [project("base", pin: pin)]
            let item = published("base", pin: pin)
            workspace.catalog.resources = [try XCTUnwrap(item.resource)]
            var first = item.document
            first.body = "First save awaiting acknowledgement"
            workspace.edits.stageDocumentSave(item, document: first)
            let firstFlush = Task { await workspace.flushPendingChanges() }
            await writes.waitUntilStarted()

            var nextItem = item
            if addsAnotherDocument {
                let original = try XCTUnwrap(item.resource)
                var document = original.document
                document.path = "another.md"
                let resource = MemoryResource(id: "another-memory", scope: original.scope,
                    projectId: original.projectId, projectName: original.projectName, kind: original.kind,
                    contentHash: original.contentHash, updatedAt: original.updatedAt,
                    refCommitId: original.refCommitId, contentLoaded: true, document: document, baseCommitPin: pin)
                nextItem = .init(id: resource.id, resource: resource, draft: nil, inherited: false,
                    projectContextId: "project", editBasePin: pin)
                workspace.catalog.resources.append(resource)
            }
            var next = nextItem.document
            next.body = "New edit while the first save waits"
            workspace.edits.stageDocumentSave(nextItem, document: next)
            // Cancel the debounce before releasing the first write. The second
            // explicit flush waits behind the same mutation gate.
            let secondFlush = Task { await workspace.flushPendingChanges() }
            await writes.acknowledge()
            let firstSucceeded = await firstFlush.value
            XCTAssertFalse(firstSucceeded, "Acknowledging an older edit must not allow the App to exit.")
            await writes.waitUntilStarted(count: 2)
            let key = MemoryDocumentSessionKey(projectId: "project", itemId: nextItem.id)
            XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.document.body, next.body)
            XCTAssertEqual(workspace.edits.pendingDocumentSaves[key]?.baseCommitId, "base")
            XCTAssertTrue(workspace.edits.pendingDocumentSaves[key]?.item.editBasePin === pin)
            XCTAssertTrue(workspace.hasPendingChanges)
            await writes.acknowledge()
            let secondSucceeded = await secondFlush.value
            XCTAssertTrue(secondSucceeded)
            XCTAssertFalse(workspace.hasPendingChanges)
            let requests = await writes.requests
            XCTAssertEqual(requests.count, 2)
            XCTAssertTrue(requests.allSatisfy { $0.baseCommitId == "base" && $0.baseCommitCaptured })
            let bodies = requests.compactMap { request -> String? in
                if case .update(_, let content, _) = request.op { return content.content }
                return nil
            }
            XCTAssertEqual(bodies, [first.body, next.body])
        }
    }

    func testSignOutKeepsReadyPhaseUntilPendingSaveAcknowledgement() async throws {
        let writes = RecoveryWriteProbe(pauses: true)
        let started = expectation(description: "Sign out reached the pending local write")
        let workspace = WorkspaceCoordinator(storeDraft: { request in
            started.fulfill()
            return try await writes.write(request)
        })
        defer { workspace.clearAuthorityScopedWorkspace() }
        workspace.context.phase = .ready
        workspace.context.activeProjectId = "project"
        let pin = CommitBasePin(commitId: "base") { }
        workspace.context.projects = [project("base", pin: pin)]
        let item = published("base", pin: pin)
        var text = item.document
        text.body = "Unsaved before sign out"
        workspace.edits.stageDocumentSave(item, document: text)
        let signOut = Task { await workspace.signOut() }
        await fulfillment(of: [started], timeout: 1)
        XCTAssertTrue(workspace.context.isSigningOut)
        XCTAssertEqual(workspace.context.phase, .ready)
        // Reject before any remote session/config mutation. This exercises the
        // real save gate without touching credentials or the installed daemon.
        await writes.reject()
        await signOut.value
        XCTAssertFalse(workspace.context.isSigningOut)
        XCTAssertEqual(workspace.context.phase, .ready)
        XCTAssertEqual(workspace.edits.pendingDocument(for: item), text)
        XCTAssertTrue(workspace.hasPendingChanges)
    }

    func testNativeEditorIsDisabledWhileDeleteOrDiscardWaitsAndRecoversAfterFailure() async throws {
        for action in ["delete", "discard"] {
            let writes = RecoveryWriteProbe(pauses: true)
            let workspace = WorkspaceCoordinator(storeDraft: { try await writes.write($0) })
            defer { workspace.clearAuthorityScopedWorkspace() }
            workspace.context.phase = .ready
            workspace.context.activeProjectId = "project"
            let pin = CommitBasePin(commitId: "base") { }
            workspace.context.projects = [project("base", pin: pin)]
            let item = draftItem("base", pin: pin, targetId: "memory", resource: published("base", pin: pin).resource)
            workspace.edits.drafts = [try XCTUnwrap(item.draft)]
            let editor = model(item, in: workspace)
            editor.document.body = "Keep this text after rejection"
            editor.stageSave(editor.document, item: item, mode: .source)
            let finished = expectation(description: "Destructive request rejected")
            let observation = editor.$suppressesSaving.dropFirst().filter { !$0 }.sink { _ in finished.fulfill() }
            if action == "delete" { editor.moveToTrash(item: item) }
            else { editor.discard(try XCTUnwrap(item.draft), item: item) }
            await writes.waitUntilStarted()
            XCTAssertTrue(editor.suppressesSaving)
            let content = DocumentSessionView(item: item, mode: .source, model: editor)
                .environmentObject(workspace.catalog).environmentObject(workspace.context)
                .environmentObject(workspace.edits).environmentObject(workspace.feedback)
                .environmentObject(workspace.memory).environmentObject(workspace.navigation)
                .environmentObject(workspace.reviews).environmentObject(workspace.sessions)
            let host = NSHostingView(rootView: AnyView(content))
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 800, height: 600),
                styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.contentView = host
            defer { window.close() }
            host.layoutSubtreeIfNeeded()
            func textView(in view: NSView) -> CenteredTextView? {
                if let text = view as? CenteredTextView { return text }
                return view.subviews.lazy.compactMap { textView(in: $0) }.first
            }
            let text = try XCTUnwrap(textView(in: host))
            XCTAssertFalse(text.isEditable, "The actual AppKit editor must reject new typing until the destructive request finishes.")
            await writes.reject()
            await fulfillment(of: [finished], timeout: 1)
            XCTAssertFalse(editor.suppressesSaving)
            // Offscreen SwiftUI hosts defer incremental rendering. Render the
            // completed state explicitly without showing a test window.
            host.rootView = AnyView(content.id(UUID()))
            host.layoutSubtreeIfNeeded()
            XCTAssertTrue(try XCTUnwrap(textView(in: host)).isEditable)
            XCTAssertEqual(workspace.edits.pendingDocument(for: item)?.body, editor.document.body)
            XCTAssertNotNil(workspace.feedback.errorMessage)
            workspace.clearAuthorityScopedWorkspace()
            withExtendedLifetime((host, observation)) { }
        }
    }

    private func project(_ commit: String?, pin: CommitBasePin?) -> ProjectState {
        .init(id: "project", name: "Project", refCommitId: commit, refEtag: "ref", selectedOrgResourceIds: [],
            orgSelectionRevision: 0, isLoaded: true, baseCommitPin: pin)
    }

    private func published(_ commit: String?, pin: CommitBasePin?) -> MemoryListItem {
        let resource = MemoryResource(id: "memory", scope: .project, projectId: "project", projectName: "Project",
            kind: .context, contentHash: "hash", updatedAt: "now", refCommitId: commit,
            contentLoaded: true, document: .init(title: "Memory", path: "memory.md", body: "Saved"), baseCommitPin: pin)
        return .init(id: "memory", resource: resource, draft: nil, inherited: false,
            projectContextId: "project", editBasePin: pin)
    }

    private func draftItem(_ base: String?, pin: CommitBasePin?, targetId: String? = nil,
                           resource: MemoryResource? = nil) -> MemoryListItem {
        let draft = LocalDraft(id: "draft", projectId: "project", serverId: "server-draft", serverVersion: 1,
            baseCommitId: base, currentCommitId: base, freshness: .current, hasUpstreamResourceChanges: false,
            reconciliation: .unknown, reconciliationCandidateId: nil, scope: .project, kind: .context,
            targetId: targetId, status: .open, origin: .desktop, syncStatus: .synced, updatedAt: "now",
            document: .init(title: "Memory", path: "memory.md", body: "Saved"), isDeletion: false, baseCommitPin: pin)
        return .init(id: targetId ?? draft.id, resource: resource, draft: draft, inherited: false,
            projectContextId: "project", editBasePin: pin)
    }

    private func model(_ item: MemoryListItem, in workspace: WorkspaceCoordinator) -> DocumentEditorModel {
        .init(item: item, drafts: workspace.edits, context: workspace.context, feedback: workspace.feedback,
            sessions: workspace.sessions, memory: workspace.memory, reviews: workspace.reviews,
            reconciliation: workspace.reconciliation)
    }
}

private enum RecoveryError: Error { case writeRejected, refreshRejected }

private actor RecoveryWriteProbe {
    private(set) var requests: [DaemonDraftOperationRequest] = []
    private let pauses: Bool
    private var starts: [(Int, CheckedContinuation<Void, Never>)] = []
    private var completion: CheckedContinuation<DaemonDraftOperationResponse, Error>?
    init(pauses: Bool = false) { self.pauses = pauses }
    func write(_ request: DaemonDraftOperationRequest) async throws -> DaemonDraftOperationResponse {
        requests.append(request)
        let ready = starts.filter { $0.0 <= requests.count }
        starts.removeAll { $0.0 <= requests.count }
        ready.forEach { $0.1.resume() }
        if !pauses { throw RecoveryError.writeRejected }
        return try await withCheckedThrowingContinuation { completion = $0 }
    }
    func waitUntilStarted(count: Int = 1) async {
        if requests.count >= count { return }
        await withCheckedContinuation { starts.append((count, $0)) }
    }
    func acknowledge() {
        let request = requests.last
        let target: String?
        if let request, case .update(let id, _, _) = request.op { target = id }
        else { target = nil }
        let draftId = request?.draftId ?? target.map { "draft-" + $0 } ?? "draft"
        completion?.resume(returning: .init(localOperationId: "operation", draftId: draftId, queued: true, syncStatus: .queued))
        completion = nil
    }
    func reject() {
        completion?.resume(throwing: RecoveryError.writeRejected)
        completion = nil
    }
    func detail(_ draftId: String) throws -> DaemonDraftDetail {
        let request = try XCTUnwrap(requests.last)
        let target: String?
        if case .update(let id, _, _) = request.op { target = id }
        else { target = nil }
        return .init(draft: .init(draftId: draftId, projectId: request.projectId,
            serverDraftId: nil, serverVersion: 0, baseCommitId: request.baseCommitId,
            currentCommitId: request.baseCommitId, freshness: .current, hasUpstreamResourceChanges: false,
            reconciliation: .unknown, reconciliationCandidateId: nil, scope: .project,
            resourceKind: .memory, targetId: target, path: "memory.md", status: .open,
            createdAt: "now", updatedAt: "now", pendingOperationCount: 1, failedOperationCount: 0),
            operations: [.init(localOperationId: "operation", resourceKind: .memory,
                operation: request.op, source: .desktop, syncStatus: .queued, lastError: nil,
                createdAt: "now", updatedAt: "now")])
    }
}
