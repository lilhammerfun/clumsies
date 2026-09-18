import XCTest
@testable import Clumsies

final class LiveWorkspaceIntegrationTests: XCTestCase {
    private enum DraftSyncBarrierDecision: Equatable {
        case complete
        case wait
        case failed(Int)
    }

    private enum DraftSyncBarrierError: LocalizedError {
        case failedOperations(Int)
        case timedOut

        var errorDescription: String? {
            switch self {
            case let .failedOperations(count):
                "Draft sync reported \(count) failed operation(s)."
            case .timedOut:
                "Draft sync did not drain its pending operations within 30 seconds."
            }
        }
    }

    func testDraftSyncBarrierOnlyCompletesForAHealthyEmptyQueue() {
        XCTAssertEqual(
            Self.draftSyncBarrierDecision(pendingOperationCount: 0, failedOperationCount: 0),
            .complete
        )
        XCTAssertEqual(
            Self.draftSyncBarrierDecision(pendingOperationCount: 1, failedOperationCount: 0),
            .wait
        )
        XCTAssertEqual(
            Self.draftSyncBarrierDecision(pendingOperationCount: 0, failedOperationCount: 1),
            .failed(1)
        )
    }

    func testLoadsAuthenticatedWorkspaceThroughDaemon() async throws {
        guard ProcessInfo.processInfo.environment["CLUMSIES_RUN_LIVE_TESTS"] == "1" else {
            throw XCTSkip("Set CLUMSIES_RUN_LIVE_TESTS=1 to exercise the local daemon and configured Server.")
        }

        let daemon = DaemonXPCClient()
        let loader = WorkspaceLoader(
            daemon: daemon,
            bootstrap: DaemonBootstrapController(),
            server: ServerClient(daemon: daemon)
        )
        let snapshot = try await loader.load()

        XCTAssertFalse(snapshot.account.email.isEmpty)
        XCTAssertFalse(snapshot.organization.name.isEmpty)
        XCTAssertFalse(snapshot.projects.isEmpty)
        XCTAssertTrue(snapshot.runtime.health.localDb.ready)
        XCTAssertEqual(snapshot.activeProjectId, snapshot.runtime.health.projectId)
        XCTAssertEqual(snapshot.projects.filter(\.isLoaded).count, 1)
        XCTAssertTrue(snapshot.resources.allSatisfy {
            $0.scope == .org || $0.projectId == snapshot.activeProjectId
        })

        if let resource = snapshot.resources.first(where: { !$0.contentLoaded }) {
            let loaded = try await loader.loadContent(for: resource)
            XCTAssertTrue(loaded.contentLoaded)
            XCTAssertEqual(loaded.id, resource.id)
            XCTAssertEqual(loaded.document.path, resource.document.path)
        }

        if let project = snapshot.projects.first(where: { $0.id != snapshot.activeProjectId }) {
            let loaded = try await loader.loadProject(id: project.id, name: project.name)
            XCTAssertTrue(loaded.state.isLoaded)
            XCTAssertTrue(loaded.resources.allSatisfy { $0.projectId == project.id })
        }
    }

    @MainActor
    func testNativeDraftAndBundleLifecycle() async throws {
        guard ProcessInfo.processInfo.environment["CLUMSIES_RUN_LIVE_TESTS"] == "1" else {
            throw XCTSkip("Set CLUMSIES_RUN_LIVE_TESTS=1 to exercise native write paths.")
        }

        let store = WorkspaceStore()
        await store.reload()
        XCTAssertEqual(store.phase, .ready)

        for kind in [MemoryKind.context, .rules, .workflows] {
            try await exerciseDraft(kind: kind, store: store)
        }

        let originalBundleIds = Set(store.bundles.map(\.id))
        await store.createBundle()
        let createdBundle = try XCTUnwrap(store.bundles.first { !originalBundleIds.contains($0.id) })
        do {
            let name = "Native integration \(UUID().uuidString.prefix(8))"
            try await store.updateBundle(
                createdBundle,
                name: name,
                description: "Temporary native client integration test.",
                resourceIds: []
            )
            XCTAssertEqual(store.bundles.first { $0.id == createdBundle.id }?.name, name)
            let currentBundle = try XCTUnwrap(store.bundles.first { $0.id == createdBundle.id })
            await store.deleteBundle(currentBundle)
            XCTAssertFalse(store.bundles.contains { $0.id == createdBundle.id })
        } catch {
            if let currentBundle = store.bundles.first(where: { $0.id == createdBundle.id }) {
                await store.deleteBundle(currentBundle)
            }
            throw error
        }
        try await waitForDraftSync(daemon: DaemonXPCClient())
    }

    @MainActor
    private func exerciseDraft(kind: MemoryKind, store: WorkspaceStore) async throws {
        let originalDraftIds = Set(store.drafts.map(\.id))
        await store.createMemory(kind: kind, scope: .org)
        let createdDraft = try XCTUnwrap(store.drafts.first { !originalDraftIds.contains($0.id) })

        do {
            var document = createdDraft.document
            document.title = "Native \(kind.singularTitle) integration test"
            document.body = "Temporary content \(UUID().uuidString)"
            let item = MemoryListItem(
                id: createdDraft.id,
                resource: nil,
                draft: createdDraft,
                inherited: false,
                projectContextId: createdDraft.projectId
            )
            try await store.save(item, document: document)
            let updatedDraft = try XCTUnwrap(store.drafts.first { $0.id == createdDraft.id })
            XCTAssertEqual(updatedDraft.document.body, document.body)
            XCTAssertNotEqual(updatedDraft.syncStatus, .failed)
            await store.discard(updatedDraft)
            XCTAssertFalse(store.drafts.contains { $0.id == createdDraft.id })
        } catch {
            if let currentDraft = store.drafts.first(where: { $0.id == createdDraft.id }) {
                await store.discard(currentDraft)
            }
            throw error
        }
    }

    private static func draftSyncBarrierDecision(
        pendingOperationCount: Int,
        failedOperationCount: Int
    ) -> DraftSyncBarrierDecision {
        if failedOperationCount > 0 {
            return .failed(failedOperationCount)
        }
        return pendingOperationCount == 0 ? .complete : .wait
    }

    @MainActor
    private func waitForDraftSync(daemon: DaemonXPCClient) async throws {
        let maximumAttempts = 300
        for attempt in 0..<maximumAttempts {
            let status = try await daemon.syncStatus()
            switch Self.draftSyncBarrierDecision(
                pendingOperationCount: status.pendingOperationCount,
                failedOperationCount: status.failedOperationCount
            ) {
            case .complete:
                return
            case .wait:
                if attempt + 1 < maximumAttempts {
                    try await Task.sleep(for: .milliseconds(100))
                }
            case let .failed(count):
                throw DraftSyncBarrierError.failedOperations(count)
            }
        }
        throw DraftSyncBarrierError.timedOut
    }
}
