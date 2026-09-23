import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class ReviewUpdateTests: XCTestCase {
    enum Failure: Error { case offline }

    func testRequestSubmitsIdenticalRemoteAndDraftWithoutOpeningReconciliation() async {
        let clean = fixture().candidates[0]
        let identical = DraftReconciliationCandidate(candidateId: "identical", draftId: "draft-identical",
            draftVersion: 2, baseCommitId: "base", currentCommitId: "shared", status: .clean,
            baseState: clean.baseState, currentState: clean.draftState, draftState: clean.draftState,
            proposedState: clean.draftState, conflicts: [], resultHash: nil, valid: true,
            createdAt: clean.createdAt, invalidatedAt: nil)
        for candidates in [[identical], [identical, clean]] {
            var submissions = 0
            let model = ReviewRequestModel(initialTitle: "Shared changes", loadCandidates: { candidates },
                onSubmit: { _, _, reconciliations, _ in
                    submissions += 1
                    if submissions == 1 { throw ReviewRequestError.reconciliationRequired }
                    XCTAssertEqual(reconciliations.map(\.candidate.candidateId), candidates.map(\.candidateId))
                    XCTAssertTrue(reconciliations.allSatisfy { $0.resolvedState == nil })
                })
            let submitted = await model.submit()
            XCTAssertTrue(submitted, "a clean comparison must not ask the user to resolve a conflict")
            XCTAssertEqual(submissions, 2)
            XCTAssertTrue(model.reconciliationCandidates.isEmpty)
        }
    }

    func testRequestReportsNoChangesAndKeepsRealConflictChoicesAfterFailure() async {
        let empty = ReviewRequestModel(initialTitle: "Already published", loadCandidates: {
            XCTFail("unchanged drafts must not open the conflict flow")
            return []
        }, onSubmit: { _, _, _, _ in throw ReviewRequestError.noChanges })
        let submitted = await empty.submit()
        XCTAssertFalse(submitted)
        XCTAssertNotNil(empty.noticeMessage)
        XCTAssertNil(empty.errorMessage)
        XCTAssertTrue(empty.reconciliationCandidates.isEmpty)

        let candidates = fixture().candidates
        var acceptsRemote = false
        let form = ReviewRequestModel(initialTitle: "Keep this title", loadCandidates: { candidates },
            onSubmit: { _, _, choices, _ in
                if choices.isEmpty { throw ReviewRequestError.reconciliationRequired }
                if acceptsRemote { throw ReviewRequestError.noChanges }
                throw Failure.offline
            })
        form.description = "Keep this explanation"
        let requested = await form.submit()
        XCTAssertFalse(requested)
        XCTAssertEqual(form.activeConflictCandidate?.candidateId, "conflict")
        form.resolvedStatesByCandidateId["conflict"] = candidates[1].draftState
        let saved = await form.submitBatch()
        XCTAssertFalse(saved)
        XCTAssertEqual(form.resolvedStatesByCandidateId["conflict"], candidates[1].draftState)
        XCTAssertEqual(form.description, "Keep this explanation")
        XCTAssertNotNil(form.errorMessage)
        acceptsRemote = true
        let unchanged = await form.submitBatch()
        XCTAssertFalse(unchanged)
        XCTAssertTrue(form.reconciliationCandidates.isEmpty)
        XCTAssertNotNil(form.noticeMessage)
        XCTAssertNil(form.errorMessage)
    }

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
        XCTAssertFalse(model.canApply)
        let candidate = plan.candidates[1]
        XCTAssertFalse(model.canApply, "unresolved marker sections cannot be confirmed")
        let resolution = ReconciliationResourceState(exists: true,
            resource: candidate.draftState.resource, content: .init(description: nil, content: "Combined"))
        model.setResolution(resolved(candidate, choosing: resolution), for: candidate.candidateId)
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
        let update = try XCTUnwrap(workspace.reviews.updates[review.id])
        let candidate = plan.candidates[1]
        update.setResolution(resolved(candidate, choosing: candidate.draftState), for: candidate.candidateId)
        let detail = ReviewDetailModel(reviewId: review.id, context: workspace.context,
            feedback: workspace.feedback, reviews: workspace.reviews, fetchDetail: { _ in plan.detail })
        await detail.refreshDetail()
        XCTAssertTrue(workspace.reviews.updates[review.id] === update)
        XCTAssertEqual(update.resolutions[candidate.candidateId]?.state, candidate.draftState)
        workspace.reviews.selectedReviewId = "another-review"
        workspace.reviews.selectedReviewId = review.id
        XCTAssertTrue(workspace.reviews.beginUpdate(review) === update)
        XCTAssertEqual(update.resolutions[candidate.candidateId]?.state, candidate.draftState)
        workspace.clearAuthorityScopedWorkspace()
        XCTAssertNil(workspace.reviews.updates[review.id])
        XCTAssertTrue(update.resolutions.isEmpty)
    }

    func testDetailRefreshHidesConnectionNoiseButReportsLostAccess() async {
        let plan = fixture()
        let workspace = WorkspaceCoordinator()
        let review = WorkspaceLoader.mapReview(plan.detail.review)
        workspace.reviews.reviews = [review]
        var failure: Error = URLError(.notConnectedToInternet)
        let model = ReviewDetailModel(reviewId: review.id, context: workspace.context,
            feedback: workspace.feedback, reviews: workspace.reviews,
            fetchDetail: { _ in throw failure })
        model.detail = plan.detail
        model.commentDraft = "Keep this comment."
        await model.refreshDetail()
        XCTAssertNotNil(model.detail)
        XCTAssertNil(model.loadError)
        XCTAssertNil(workspace.feedback.errorMessage)
        failure = ServerClientError.response(status: 403, message: "PRIVATE_BODY")
        await model.refreshDetail()
        XCTAssertEqual(workspace.feedback.errorMessage, ClientFailure.forbidden.message)
        XCTAssertEqual(model.commentDraft, "Keep this comment.")
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

    func testInlineChoicesRetainAutomaticChangesAndResetWithoutAResultEditor() async throws {
        let plan = fixture()
        let model = ReviewUpdateModel(review: WorkspaceLoader.mapReview(plan.detail.review),
            prepare: { plan }, apply: { _, _ in plan.detail })
        await model.load()
        let candidate = plan.candidates[1]
        let host = NSHostingView(rootView: ScrollView {
            ReviewUpdateView(model: model, draftId: candidate.draftId) { Text("Current file") }
                .padding(20)
        })
        host.sizingOptions = []
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 850, height: 600),
            styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        func editors(_ view: NSView) -> [NSTextView] {
            (view as? NSTextView).map { [$0] } ?? view.subviews.flatMap(editors)
        }
        for _ in 0..<3 {
            host.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertNil(window.attachedSheet)
        XCTAssertTrue(editors(host).isEmpty, "Review must not show an editable merge-result area")
        XCTAssertFalse(model.canApply)
        var resolution = try XCTUnwrap(model.resolutions[candidate.candidateId])
        let section = try XCTUnwrap(resolution.sections.first)
        resolution.chooseContent(section.proposed, in: section)
        model.setResolution(resolution, for: candidate.candidateId)
        XCTAssertTrue(model.canApply)
        XCTAssertEqual(model.resolutions[candidate.candidateId]?.text, "Proposed\n")
        XCTAssertEqual(model.resolutions["clean"]?.state, plan.candidates[0].proposedState)
        let result = await model.submit()
        XCTAssertEqual(result?.review.reviewId, plan.detail.review.reviewId)
        model.setResolution(DraftResolution(candidate: candidate), for: candidate.candidateId)
        XCTAssertFalse(model.canApply)
        XCTAssertFalse(model.hasEdits)
        XCTAssertEqual(model.resolutions[candidate.candidateId]?.sections.count, 1)
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

    func testBadgesDistinguishSavedRebaseFromCleanPreviewAndNewRemoteChanges() throws {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let saved = try decoder.decode(DraftCoordination.self, from: Data("""
        {"freshness":"current","current_commit_id":"remote","has_upstream_resource_changes":false,
         "reconciliation":"unknown","candidate_id":null,"auto_rebased":true}
        """.utf8))
        XCTAssertEqual(ReviewReconciliationState.resolve(freshness: saved.freshness,
            reconciliation: saved.reconciliation, autoRebased: saved.autoRebased == true), .autoRebased)
        XCTAssertEqual(ReviewReconciliationState.resolve(freshness: .behind,
            reconciliation: .clean, autoRebased: false), .checking,
            "a computed clean candidate is not a saved rebase")
        XCTAssertEqual(ReviewReconciliationState.resolve(freshness: .behind,
            reconciliation: .conflicts, autoRebased: true), .conflict,
            "a newer conflict takes precedence over past automatic updates")
        XCTAssertNil(ReviewReconciliationState.resolve(freshness: .current,
            reconciliation: .unknown, autoRebased: false))
    }

    func testPreparationPublishesSavedResultOnRetryWithoutOfferingAnotherSave() async {
        let plan = fixture()
        let completed = ReviewUpdatePlan(detail: plan.detail, candidates: [])
        var offline = true
        var loadResults: [Bool] = []
        var manualSaves = 0
        let model = ReviewUpdateModel(review: WorkspaceLoader.mapReview(plan.detail.review),
            prepare: { if offline { throw Failure.offline }; return completed },
            apply: { _, _ in manualSaves += 1; return completed.detail })
        model.didLoad = { loadResults.append(model.plan != nil && model.errorMessage == nil) }
        await model.load()
        XCTAssertNotNil(model.errorMessage)
        XCTAssertNil(model.plan)
        offline = false
        await model.load(restart: true)
        XCTAssertEqual(loadResults, [false, true])
        XCTAssertTrue(model.candidates.isEmpty)
        XCTAssertFalse(model.canApply)
        let result = await model.submit()
        XCTAssertNil(result)
        XCTAssertEqual(manualSaves, 0)
    }

    func testReviewerCanInspectConflictsButCannotSubmitAuthorChoices() async {
        let plan = fixture()
        var saves = 0
        let model = ReviewUpdateModel(review: WorkspaceLoader.mapReview(plan.detail.review),
            canResolveConflicts: false, prepare: { plan },
            apply: { _, _ in saves += 1; return plan.detail })
        await model.load()
        let candidate = plan.candidates[1]
        model.setResolution(resolved(candidate, choosing: candidate.draftState), for: candidate.candidateId)
        XCTAssertFalse(model.hasEdits)
        XCTAssertFalse(model.canApply)
        let result = await model.submit()
        XCTAssertNil(result)
        XCTAssertEqual(saves, 0)
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
