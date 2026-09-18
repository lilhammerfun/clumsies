import XCTest
@testable import Clumsies

@MainActor
final class FrontendFeatureModelTests: XCTestCase {
    func testProjectStorageIgnoresAnOldProjectResponse() async {
        let context = WorkspaceContext()
        context.activeProjectId = "first"
        let started = expectation(description: "First storage request")
        var pending: CheckedContinuation<DaemonProjectStorage, Never>?
        let model = ProjectStorageModel(context: context) { project in
            if project == "first" {
                return await withCheckedContinuation { pending = $0; started.fulfill() }
            }
            return self.storage(project)
        }
        let old = Task { await model.loadStorage() }
        await fulfillment(of: [started], timeout: 1)
        context.activeProjectId = "second"
        await model.loadStorage()
        pending?.resume(returning: storage("first"))
        await old.value
        XCTAssertEqual(model.storage?.projectId, "second")
        XCTAssertFalse(model.isWorking)
        XCTAssertNil(model.errorMessage)
    }

    func testRepositoriesIgnoreAnOldProjectFailure() async {
        let workspace = WorkspaceCoordinator()
        workspace.context.activeProjectId = "first"
        let started = expectation(description: "First repository request")
        var pending: CheckedContinuation<[DaemonProjectBinding], Error>?
        let model = ProjectRepositoriesModel(context: workspace.context, projects: workspace.projects) { project in
            if project == "first" {
                return try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
            }
            return [.init(serverUrl: "https://example.com", workspaceRoot: "/second", projectId: project,
                revision: 1, createdAt: "now", updatedAt: "now")]
        }
        let old = Task { await model.load() }
        await fulfillment(of: [started], timeout: 1)
        workspace.context.activeProjectId = "second"
        await model.load()
        pending?.resume(throwing: TestFailure.delayed)
        await old.value
        XCTAssertEqual(model.bindings.map(\.projectId), ["second"])
        XCTAssertNil(model.errorMessage)
        XCTAssertFalse(model.isLoading)
    }

    func testMemberSearchRejectsOldResponseEvenWhenQueryReturnsToSameText() async {
        let context = WorkspaceContext()
        let administration = AdministrationModel(context: context, onWorkspaceChanged: {})
        let started = expectation(description: "Old search")
        var pending: CheckedContinuation<ListResponse<UserReference>, Never>?
        var count = 0
        let model = ProjectMemberPickerModel(projectId: "project", administration: administration) { _, _ in
            count += 1
            if count == 1 {
                return await withCheckedContinuation { pending = $0; started.fulfill() }
            }
            return .init(items: [self.user("latest")], pageInfo: .init(nextCursor: "next", hasMore: true))
        }
        model.query = "alice"
        let old = Task { await model.loadMembers() }
        await fulfillment(of: [started], timeout: 1)
        model.query = "bob"
        model.query = "alice"
        await model.loadMembers()
        pending?.resume(returning: .init(items: [user("old")], pageInfo: .init(nextCursor: nil, hasMore: false)))
        await old.value
        XCTAssertEqual(model.members.map(\.userId), ["latest"])
        XCTAssertEqual(model.nextCursor, "next")
        context.authorityGeneration = UUID()
        XCTAssertTrue(model.members.isEmpty)
        XCTAssertNil(model.nextCursor)
    }

    func testReviewDetailDoesNotRestoreAnErrorAfterAuthorityReset() async {
        let workspace = WorkspaceCoordinator()
        let started = expectation(description: "Review request")
        var pending: CheckedContinuation<ReviewDetail, Error>?
        let model = ReviewDetailModel(reviewId: "review", context: workspace.context, feedback: workspace.feedback,
            reconciliation: workspace.reconciliation, reviews: workspace.reviews) { _ in
                try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
            }
        let old = Task { await model.load() }
        await fulfillment(of: [started], timeout: 1)
        workspace.clearAuthorityScopedWorkspace()
        pending?.resume(throwing: TestFailure.delayed)
        await old.value
        XCTAssertNil(model.detail)
        XCTAssertNil(model.loadError)
        XCTAssertFalse(model.loading)
        XCTAssertNil(workspace.feedback.errorMessage)
    }

    func testReviewRequestRetriesAfterEmptyPreflightAndKeepsFormOnFailure() async {
        var submissions = 0
        var preflights = 0
        let model = ReviewRequestModel(initialTitle: "  Review title  ", loadCandidates: {
            preflights += 1
            return []
        }, onSubmit: { title, description, reconciliations in
            submissions += 1
            XCTAssertEqual(title, "Review title")
            XCTAssertEqual(description, "Description")
            XCTAssertTrue(reconciliations.isEmpty)
            if submissions == 1 { throw ReviewRequestError.reconciliationRequired }
            throw TestFailure.delayed
        })
        model.description = " Description "
        let submitted = await model.submit()
        XCTAssertFalse(submitted)
        XCTAssertEqual(submissions, 2)
        XCTAssertEqual(preflights, 1)
        XCTAssertEqual(model.title, "  Review title  ")
        XCTAssertEqual(model.description, " Description ")
        XCTAssertNotNil(model.errorMessage)
        XCTAssertFalse(model.isSubmitting)
    }

    func testDocumentEditorAdoptsRenameWithoutLosingDirtyBody() {
        let workspace = WorkspaceCoordinator()
        let resource = MemoryResource(id: "memory", scope: .org, projectId: nil, projectName: nil,
            kind: .context, contentHash: "hash", updatedAt: "now", refCommitId: "commit",
            contentLoaded: true, document: .init(title: "Before", path: "before.md", body: "Shared body"))
        let item = MemoryListItem(id: resource.id, resource: resource, draft: nil, inherited: true, projectContextId: "project")
        let model = DocumentEditorModel(item: item, drafts: workspace.edits, context: workspace.context,
            feedback: workspace.feedback, sessions: workspace.sessions, memory: workspace.memory,
            reviews: workspace.reviews, reconciliation: workspace.reconciliation)
        model.document.body = "Unsaved local body"
        model.adoptAuthoritativeDocument(.init(title: "After", path: "after.md", body: "New shared body"))
        XCTAssertEqual(model.document.title, "After")
        XCTAssertEqual(model.document.path, "after.md")
        XCTAssertEqual(model.document.body, "Unsaved local body")
        XCTAssertEqual(model.authoritativeDocument.body, "New shared body")
    }

    func testBulkRenameStopsWhenProjectChangesDuringFirstWrite() async {
        let workspace = WorkspaceCoordinator()
        workspace.context.activeProjectId = "first"
        let resource = MemoryResource(id: "memory", scope: .org, projectId: nil, projectName: nil,
            kind: .context, contentHash: "hash", updatedAt: "now", refCommitId: "commit",
            contentLoaded: true, document: .init(title: "Note", path: "old/note.md", body: "Body"))
        let item = MemoryListItem(id: resource.id, resource: resource, draft: nil, inherited: true, projectContextId: "first")
        var writes = 0
        let model = MemoryFileOperationsModel(context: workspace.context, feedback: workspace.feedback,
            drafts: workspace.edits, projects: workspace.projects, rename: { _, _ in
                writes += 1
                workspace.context.activeProjectId = "second"
            })
        await model.renameDirectory(.init(changes: [
            .init(item: item, newPath: "new/a.md"), .init(item: item, newPath: "new/b.md")
        ]))
        XCTAssertEqual(writes, 1)
        XCTAssertNil(model.directoryOperationProgress)
        XCTAssertNil(workspace.feedback.errorMessage)
    }

    func testActivityFragmentCannotReappearAfterRunIsCleared() async {
        let model = ActivityFragmentModel()
        let started = expectation(description: "Fragment request")
        var pending: CheckedContinuation<RecallFragment, Never>?
        let fragment = RecallFragment(action: nil, unitKey: "unit", resourceId: "memory", scope: nil,
            path: "a.md", headingPath: [], content: "", finalRank: nil, truncated: true)
        let old = Task {
            await model.load(fragment: fragment, runId: "old") {
                await withCheckedContinuation { pending = $0; started.fulfill() }
            }
        }
        await fulfillment(of: [started], timeout: 1)
        await model.load(fragment: fragment, runId: nil) { XCTFail("No fetch without a run"); return fragment }
        pending?.resume(returning: fragment)
        await old.value
        XCTAssertNil(model.fullFragment)
        XCTAssertFalse(model.isLoading)
        XCTAssertFalse(model.loadFailed)
    }

    func testDiagnosticsPaginationCannotReplaceNewProjectCursor() async {
        let started = expectation(description: "Old project page")
        var pending: CheckedContinuation<RetrievalRunListResponse, Never>?
        let model = RetrievalDiagnosticsModel(daemon: DaemonXPCClient(), fetchRuns: { request in
            if request.cursor != nil {
                return await withCheckedContinuation { pending = $0; started.fulfill() }
            }
            return .init(items: [], nextCursor: request.projectId == "old" ? "old-next" : "new-next")
        })
        await model.load(projectId: "old")
        let old = Task { await model.loadMore() }
        await fulfillment(of: [started], timeout: 1)
        await model.load(projectId: "new")
        pending?.resume(returning: .init(items: [], nextCursor: "obsolete"))
        await old.value
        XCTAssertEqual(model.nextCursor, "new-next")
        XCTAssertFalse(model.isLoadingMore)
        XCTAssertNil(model.errorMessage)
    }

    func testRecoverySnapshotCannotRestoreSignedOutSessionState() async {
        let started = expectation(description: "Recovery snapshot")
        var pending: CheckedContinuation<NativeAdministratorRecoverySnapshot, Never>?
        let model = NativeAdministratorRecoveryState { _ in
            await withCheckedContinuation { pending = $0; started.fulfill() }
        }
        model.retain(NativeAuthenticatedSession(serverURL: URL(string: "https://example.com")!,
            currentUser: .init(user: user("admin"), org: .init(orgId: "org", name: "Org"), projects: [],
                defaultProjectId: nil, capabilities: ["admin:write"]),
            accessToken: "test", refreshToken: "test", transport: URLSession(configuration: .ephemeral)))
        let old = Task { await model.load() }
        await fulfillment(of: [started], timeout: 1)
        model.clear()
        let check = AdminHealthCheck(status: .ok, message: "Ready")
        pending?.resume(returning: .init(health: .init(status: .ok, version: "test", database: check,
            schema: check, commitService: check, oidc: check), members: [], tokens: []))
        await old.value
        XCTAssertFalse(model.isAuthenticated)
        XCTAssertFalse(model.isLoading)
        XCTAssertNil(model.health)
        XCTAssertNil(model.errorMessage)
    }

    private func storage(_ project: String) -> DaemonProjectStorage {
        .init(authorityKey: "authority", projectId: project, mode: .standard,
            selectedRootPath: "/cache", managedRootPath: "/cache/managed", activeGenerationPath: nil,
            searchIndexPath: "/cache/index", availability: .ready, locationRevision: 1,
            sizeBytes: 0, activeMoveId: nil, issueCode: nil, diagnostic: nil)
    }

    private func user(_ id: String) -> UserReference {
        .init(userId: id, email: "\(id)@example.com", displayName: nil, avatarUrl: nil, role: "member")
    }
}

private enum TestFailure: Error { case delayed }
