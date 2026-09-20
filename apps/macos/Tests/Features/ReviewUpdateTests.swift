import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class ReviewUpdateTests: XCTestCase {
    enum Failure: Error { case offline }

    func testWholeReviewApplyRequiresResolutionAndRetainsEditsAfterFailure() async {
        let plan = fixture()
        var requests: [CreateReviewUpdateRequest] = []
        let model = ReviewUpdateModel(review: WorkspaceLoader.mapReview(plan.detail.review),
            prepare: { plan }, apply: { _, request in
                requests.append(request)
                throw Failure.offline
            })
        await model.load()
        XCTAssertEqual(model.candidates.count, 2)
        XCTAssertEqual(model.selectedCandidateId, "conflict")
        XCTAssertFalse(model.canApply)
        let candidate = plan.candidates[1]
        XCTAssertFalse(model.canApply, "unresolved marker sections cannot be confirmed")
        let resolution = ReconciliationResourceState(exists: true,
            resource: candidate.draftState.resource, content: .init(description: nil, content: "Combined"))
        model.setResolution(resolved(candidate, choosing: resolution), for: candidate.candidateId)
        model.selectedCandidateId = "clean"
        XCTAssertTrue(model.canApply)
        let result = await model.submit()
        XCTAssertNil(result)
        XCTAssertNotNil(model.errorMessage)
        XCTAssertEqual(model.resolutions[candidate.candidateId]?.state, resolution)
        XCTAssertTrue(model.resolutions[candidate.candidateId]?.canSave == true)
        XCTAssertTrue(model.canApply, "a failed write keeps the complete retryable form")
        XCTAssertEqual(requests.count, 1)
        XCTAssertEqual(requests[0].drafts.map(\.draftId), ["draft-clean", "draft-conflict", "draft-current"])
        XCTAssertEqual(requests[0].drafts[1].resolvedState, resolution)
        XCTAssertNil(requests[0].drafts[2].candidateId)
        var edited = resolved(candidate, choosing: candidate.draftState)
        edited.editContent(candidate.mergePreview!.state.content!.primaryText)
        model.setResolution(edited, for: candidate.candidateId)
        XCTAssertFalse(model.canApply, "unresolved markers cannot be saved after manual editing")
    }

    func testLatePlanDoesNotRestorePrivateStateAfterInvalidation() async {
        let plan = fixture()
        let started = expectation(description: "Checking review")
        var pending: CheckedContinuation<ReviewUpdatePlan, Error>?
        let model = ReviewUpdateModel(review: WorkspaceLoader.mapReview(plan.detail.review),
            prepare: {
                try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
            }, apply: { _, _ in plan.detail })
        let task = Task { await model.load() }
        await fulfillment(of: [started], timeout: 1)
        model.invalidate()
        pending?.resume(returning: plan)
        await task.value
        XCTAssertNil(model.plan)
        XCTAssertTrue(model.resolutions.isEmpty)
        XCTAssertFalse(model.isLoading)
    }

    func testConflictSectionsPreserveIndependentChangesAndUnicode() {
        let text = "自动合并的前文\n<<<<<<< ours\n共享内容\n||||||| original\n原文\n=======\n提议内容\n>>>>>>> theirs\n自动合并的后文\n"
        let sections = ContentConflictSection.parse(text, markerLength: 7)
        XCTAssertEqual(sections.count, 1)
        XCTAssertEqual(sections.first?.base, "原文\n")
        XCTAssertEqual(sections.first?.shared, "共享内容\n")
        XCTAssertEqual(sections.first?.proposed, "提议内容\n")
        for version in [sections[0].shared, sections[0].proposed] {
            let diff = UnifiedDiffPresentation(model: .make(original: sections[0].base, modified: version))
            let lines = diff.blocks.flatMap(\.lines)
            XCTAssertEqual(lines.filter { $0.kind == .removal }.map(\.text), ["原文"])
            XCTAssertEqual(lines.filter { $0.kind == .insertion }.map(\.text), [String(version.dropLast())])
        }
        let resolved = (text as NSString).replacingCharacters(in: sections[0].range, with: sections[0].proposed)
        XCTAssertEqual(resolved, "自动合并的前文\n提议内容\n自动合并的后文\n")
        XCTAssertFalse(ContentConflictSection.hasMarkers(in: resolved, length: 7))
        XCTAssertTrue(ContentConflictSection.hasMarkers(in: text, length: 7))
        XCTAssertTrue(ContentConflictSection.parse(text, markerLength: 8).isEmpty,
                      "literal marker-like input is not a generated conflict")

        let deletion = ContentConflictSection.parse(
            "<<<<<<< ours\n||||||| original\n原文\n=======\n草稿\n>>>>>>> theirs", markerLength: 7)
        XCTAssertEqual(deletion.first?.base, "原文\n")
        XCTAssertEqual(deletion.first?.shared, "")
        XCTAssertEqual(deletion.first?.proposed, "草稿\n")
    }

    func testDetailRefreshPreservesWorkflowAndAuthorityResetClearsIt() async throws {
        let plan = fixture()
        let workspace = WorkspaceCoordinator()
        workspace.context.account = plan.detail.review.author
        let review = WorkspaceLoader.mapReview(plan.detail.review)
        workspace.reviews.reviews = [review]
        workspace.reviews.beginUpdate(review)
        let update = try XCTUnwrap(workspace.reviews.update)
        let candidate = plan.candidates[1]
        update.setResolution(resolved(candidate, choosing: candidate.draftState), for: candidate.candidateId)
        let detail = ReviewDetailModel(reviewId: review.id, context: workspace.context,
            feedback: workspace.feedback, reviews: workspace.reviews, fetchDetail: { _ in plan.detail })
        await detail.refreshDetail()
        XCTAssertTrue(workspace.reviews.update === update)
        XCTAssertEqual(update.resolutions[candidate.candidateId]?.state, candidate.draftState)
        workspace.clearAuthorityScopedWorkspace()
        XCTAssertNil(workspace.reviews.update)
        XCTAssertTrue(update.resolutions.isEmpty)
    }

    func testExplicitRestartPreservesEditsOnFailureAndResetsThemOnlyAfterSuccess() async {
        let plan = fixture()
        var offline = false
        let model = ReviewUpdateModel(review: WorkspaceLoader.mapReview(plan.detail.review),
            prepare: { if offline { throw Failure.offline }; return plan },
            apply: { _, _ in plan.detail })
        await model.load()
        model.setResolution(resolved(plan.candidates[1], choosing: plan.candidates[1].draftState), for: "conflict")
        offline = true
        await model.load(restart: true)
        XCTAssertTrue(model.hasEdits)
        XCTAssertEqual(model.resolutions["conflict"]?.state, plan.candidates[1].draftState)
        offline = false
        await model.load(restart: true)
        XCTAssertFalse(model.hasEdits)
        XCTAssertFalse(model.canApply)
        XCTAssertNil(model.errorMessage)
    }

    func testConflictEditorHasUsableSpaceInWholeReviewWorkspace() async throws {
        let plan = fixture()
        let model = ReviewUpdateModel(review: WorkspaceLoader.mapReview(plan.detail.review),
            prepare: { plan }, apply: { _, _ in plan.detail })
        await model.load()
        let host = NSHostingView(rootView: ReviewUpdateView(model: model, onCancel: {}, onApplied: { _ in })
            .frame(width: 1000, height: 720).background(Color(nsColor: .windowBackgroundColor)))
        host.frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled, .resizable],
            backing: .buffered, defer: false)
        window.contentView = host
        for _ in 0..<3 {
            host.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(50))
        }
        func textViews(_ view: NSView) -> [NSTextView] {
            (view as? NSTextView).map { [$0] } ?? view.subviews.flatMap(textViews)
        }
        let editor = try XCTUnwrap(textViews(host).first)
        XCTAssertGreaterThan(try XCTUnwrap(editor.enclosingScrollView).bounds.height, 80)
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        try data.write(to: URL(fileURLWithPath: "/tmp/clumsies-review-update-preview.png"))
        let attachment = XCTAttachment(data: data, uniformTypeIdentifier: "public.png")
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testUpdateWindowKeepsTheDetailWindowAndProtectsUnsavedEdits() async throws {
        let plan = fixture()
        let model = ReviewUpdateModel(review: WorkspaceLoader.mapReview(plan.detail.review),
            prepare: { plan }, apply: { _, _ in plan.detail })
        await model.load()
        let host = NSHostingView(rootView: Text("Review details remain here")
            .frame(maxWidth: .infinity, maxHeight: .infinity))
        host.sizingOptions = []
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 820),
            styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        let originalFrame = window.frame
        window.orderFront(nil)
        var closed = false
        let controller = ReconciliationWindowController(identity: "review", title: "Update Review",
            subtitle: model.review.title, autosaveName: "ReviewWindowTest-\(UUID().uuidString)",
            hasEdits: { model.hasEdits }, isSaving: { model.isApplying }, onClose: { closed = true })
        let originalMergeSize = try XCTUnwrap(controller.window).frame.size
        controller.install(ReviewUpdateView(model: model, onCancel: {}, onApplied: { _ in }))
        controller.showWindow(nil)
        defer {
            controller.dismiss()
            window.close()
        }
        let mergeWindow = try XCTUnwrap(controller.window)
        XCTAssertNil(window.attachedSheet)
        XCTAssertNil(mergeWindow.sheetParent)
        let content = try XCTUnwrap(mergeWindow.contentView)
        for _ in 0..<3 {
            content.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(50))
        }
        func editors(_ view: NSView) -> [NSTextView] {
            (view as? NSTextView).map { [$0] } ?? view.subviews.flatMap(editors)
        }
        XCTAssertEqual(window.frame.size, originalFrame.size)
        XCTAssertEqual(mergeWindow.frame.size, originalMergeSize)
        XCTAssertTrue(window.contentView === host)
        XCTAssertTrue(editors(host).isEmpty, "the editor must not replace the detail page")
        let editor = try XCTUnwrap(editors(content).first)
        XCTAssertGreaterThan(try XCTUnwrap(editor.enclosingScrollView).bounds.height, 80)
        let candidate = try XCTUnwrap(model.selectedCandidate)
        model.setResolution(resolved(candidate, choosing: candidate.draftState), for: candidate.candidateId)
        controller.confirmDiscard = { false }
        mergeWindow.performClose(nil)
        XCTAssertFalse(closed)
        XCTAssertTrue(mergeWindow.isVisible)
        controller.confirmDiscard = { true }
        mergeWindow.performClose(nil)
        XCTAssertTrue(closed)
    }

    func testReviewDetailStaysInsideTheWindowAfterReloadAndResize() async throws {
        let plan = fixture(description: String(repeating: "较长的 Review 说明，正文仍应在窗口内显示。", count: 10))
        let workspace = WorkspaceCoordinator()
        workspace.context.account = plan.detail.review.author
        let model = ReviewDetailModel(reviewId: plan.detail.review.reviewId,
            context: workspace.context, feedback: workspace.feedback, reviews: workspace.reviews,
            fetchDetail: { _ in plan.detail })
        model.detail = plan.detail
        model.loading = false
        model.selectedFileId = model.fileDescriptors.first?.id
        let host = NSHostingView(rootView: NavigationSplitView {
            List(["Memory", "Bundles", "Reviews", "Activity"], id: \.self) { Text($0) }
                .listStyle(.sidebar)
                .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 280)
                .safeAreaInset(edge: .bottom) { Text("Account").frame(height: 40) }
        } detail: {
            NavigationStack(path: .constant([model.reviewId])) {
                Text("Reviews")
                    .navigationDestination(for: String.self) { reviewId in
                        ReviewDetailPage(reviewId: reviewId, loadsRemoteContent: false, model: model)
                    }
            }
            .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
        }.workspaceEnvironment(workspace))
        host.sizingOptions = []
        if #available(macOS 26.0, *) { host.sceneBridgingOptions = .all }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 820),
            styleMask: [.titled, .resizable, .fullSizeContentView], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.toolbarStyle = .unified
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        func splitViews(_ view: NSView) -> [NSSplitView] {
            (view as? NSSplitView).map { [$0] } ?? view.subviews.flatMap(splitViews)
        }
        for size in [NSSize(width: 1280, height: 820), NSSize(width: 920, height: 600),
                     NSSize(width: 1280, height: 820)] {
            window.setContentSize(size)
            for _ in 0..<3 {
                host.layoutSubtreeIfNeeded()
                try await Task.sleep(for: .milliseconds(50))
            }
            model.loading = true
            try await Task.sleep(for: .milliseconds(50))
            model.detail = plan.detail
            model.loading = false
            model.diffModel = .make(original: "Original", modified: String(repeating: "Draft\n", count: 100))
            for _ in 0..<3 {
                host.layoutSubtreeIfNeeded()
                try await Task.sleep(for: .milliseconds(50))
            }
            let splits = splitViews(host)
            XCTAssertFalse(splits.isEmpty)
            for split in splits {
                let frame = split.convert(split.bounds, to: host)
                XCTAssertGreaterThanOrEqual(frame.minY, -1, "Review content moved above the window")
                XCTAssertLessThanOrEqual(frame.maxY, host.bounds.height + 1, "Review content exceeded the window")
            }
        }
    }

    private func fixture(description: String = "") -> ReviewUpdatePlan {
        let user = UserReference(userId: "author", email: "author@example.test", displayName: "Author",
            avatarUrl: nil, role: "admin")
        let coordination = DraftCoordination(freshness: .behind, currentCommitId: "shared",
            hasUpstreamResourceChanges: true, reconciliation: .conflicts, candidateId: nil)
        let stamp = "2026-09-20T05:33:00Z"
        func draft(_ id: String) -> ServerDraft {
            .init(draftId: "draft-\(id)", projectId: "project", baseCommitId: "base", author: user,
                title: id, description: "", resource: .init(scope: "org", id: id, path: "\(id).md"),
                status: "submitted", coordination: coordination, version: 2, createdAt: stamp, updatedAt: stamp)
        }
        func candidate(_ id: String, status: DraftReconciliationStatus) -> DraftReconciliationCandidate {
            let state = ReconciliationResourceState(exists: true, resource: draft(id).resource,
                content: .init(description: nil, content: "Proposed"))
            return .init(candidateId: id, draftId: "draft-\(id)", draftVersion: 2,
                baseCommitId: "base", currentCommitId: "shared", status: status,
                baseState: .init(exists: true, resource: state.resource,
                    content: .init(description: nil, content: "Original")),
                currentState: .init(exists: true, resource: state.resource,
                    content: .init(description: nil, content: "Shared")), draftState: state,
                proposedState: status == .clean ? state : nil,
                conflicts: status == .clean ? [] : [.init(kind: "content", field: "content",
                    base: "Original", current: "Shared", draft: "Proposed")],
                resultHash: nil, valid: true, createdAt: stamp, invalidatedAt: nil,
                mergePreview: status == .conflicts ? .init(state: .init(exists: true, resource: state.resource,
                    content: .init(description: nil, content:
                        "<<<<<<< ours\nShared\n||||||| original\nOriginal\n=======\nProposed\n>>>>>>> theirs\n")),
                    markerLength: 7) : nil)
        }
        let metadata = ReviewMetadata(reviewId: "review", projectId: "project", draftId: "draft-clean",
            author: user, title: "Update memory", description: description, status: "open", version: 3,
            decisionBody: nil, approvedResultHash: nil, decidedBy: nil, decidedAt: nil,
            coordination: coordination, createdAt: stamp, updatedAt: stamp)
        let detail = ReviewDetail(review: metadata, draft: draft("clean"), operations: [],
            drafts: ["clean", "conflict", "current"].map { .init(draft: draft($0), operations: []) }, comments: [])
        return ReviewUpdatePlan(detail: detail,
            candidates: [candidate("clean", status: .clean), candidate("conflict", status: .conflicts)])
    }

    private func resolved(_ candidate: DraftReconciliationCandidate,
                          choosing state: ReconciliationResourceState) -> DraftResolution {
        var resolution = DraftResolution(candidate: candidate)
        resolution.chooseFile(state)
        return resolution
    }
}
