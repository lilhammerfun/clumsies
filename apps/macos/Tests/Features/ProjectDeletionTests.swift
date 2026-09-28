import XCTest
@testable import Clumsies

@MainActor
final class ProjectDeletionTests: XCTestCase {
    func testDeletionClearsProjectBeforeRefreshAndPreservesOtherProjects() async throws {
        for active: String? in ["deleted", "kept", nil] {
            let workspace = WorkspaceCoordinator()
            let context = workspace.context
            context.phase = .ready
            context.capabilities = ["admin:write"]
            context.projects = [project("deleted"), project("kept")]
            context.projectRoles = ["deleted": .owner, "kept": .owner]
            context.activeProjectId = active
            context.loadingProjectId = "deleted"
            workspace.navigation.showsProjectSettings = active == "deleted"
            let deleted = tab("deleted")
            let kept = tab("kept")
            let org = WorkbenchTab(section: .memory, projectId: nil, itemId: "memory", mode: .preview, title: "Memory")
            workspace.navigation.tabs = [deleted, kept, org]
            workspace.navigation.activeTabId = active == "deleted" ? deleted.id : (active == "kept" ? kept.id : org.id)
            workspace.navigation.navigationBackStack = [deleted.id, kept.id]
            let resource = MemoryResource(id: "memory", scope: .org, projectId: nil, projectName: nil,
                kind: .context, contentHash: "hash", updatedAt: "now", refCommitId: nil,
                contentLoaded: true, document: .init(title: "Memory", path: "memory.md", body: "Text"))
            workspace.catalog.resources = [resource]
            for id in ["deleted", "kept"] {
                let item = MemoryListItem(id: resource.id, resource: resource, draft: nil,
                    inherited: true, projectContextId: id)
                workspace.edits.pendingDocumentSaves[.init(projectId: id, itemId: resource.id)] =
                    .init(item: item, document: resource.document, generation: UUID())
            }
            context.server = client(status: 200)
            let generation = context.workspaceReloadGeneration
            var reloaded = false
            let model = AdministrationModel(context: context, onWorkspaceChanged: {
                reloaded = true
                XCTAssertEqual(context.projects.map(\.id), ["kept"])
                XCTAssertEqual(context.activeProjectId, active == "deleted" ? nil : active)
                XCTAssertEqual(workspace.navigation.tabs, [kept, org])
                XCTAssertEqual(workspace.navigation.activeTabId, active == "deleted" ? nil : (active == "kept" ? kept.id : org.id))
                XCTAssertEqual(workspace.navigation.navigationBackStack, [kept.id])
                XCTAssertEqual(Set(workspace.edits.pendingDocumentSaves.keys.map(\.projectId)), ["kept"])
                // A skipped or offline refresh must not be required for deletion to take effect.
            })
            await model.loadProject(id: "deleted")
            var dismissed = false
            try await model.deleteAdminProject(try XCTUnwrap(model.project(id: "deleted"))) { dismissed = true }
            XCTAssertTrue(dismissed)
            XCTAssertTrue(reloaded)
            XCTAssertNil(context.projectRoles["deleted"])
            XCTAssertNil(context.loadingProjectId)
            XCTAssertNotEqual(context.workspaceReloadGeneration, generation)
            XCTAssertFalse(context.isMutatingAdministration)
            XCTAssertNotNil(model.statusMessage)
            XCTAssertNil(model.project(id: "deleted"))
            XCTAssertEqual(workspace.catalog.resources, [resource])
            XCTAssertFalse(workspace.navigation.showsProjectSettings)
        }
    }

    func testFailedDeletionKeepsTheCurrentProjectAndDoesNotReportSuccess() async throws {
        let workspace = WorkspaceCoordinator()
        let context = workspace.context
        context.phase = .ready
        context.capabilities = ["admin:write"]
        context.projects = [project("deleted")]
        context.activeProjectId = "deleted"
        workspace.navigation.tabs = [tab("deleted")]
        workspace.navigation.showsProjectSettings = true
        context.server = client(status: 500)
        let model = AdministrationModel(context: context, onWorkspaceChanged: { XCTFail("Failed deletion must not refresh") })
        await model.loadProject(id: "deleted")
        do {
            try await model.deleteAdminProject(try XCTUnwrap(model.project(id: "deleted"))) { XCTFail("Must not dismiss") }
            XCTFail("Deletion should fail")
        } catch { XCTAssertEqual(ClientFailure(error), .service) }
        XCTAssertEqual(context.activeProjectId, "deleted")
        XCTAssertEqual(context.projects.map(\.id), ["deleted"])
        XCTAssertEqual(workspace.navigation.tabs, [tab("deleted")])
        XCTAssertTrue(workspace.navigation.showsProjectSettings)
        XCTAssertNil(model.statusMessage)
        XCTAssertFalse(context.isMutatingAdministration)
        ClientServiceStatus.shared.reset()
    }

    private func project(_ id: String) -> ProjectState {
        .init(id: id, name: id, refCommitId: nil, refEtag: "", selectedOrgResourceIds: ["memory"],
            orgSelectionRevision: 1, isLoaded: true)
    }

    private func tab(_ id: String) -> WorkbenchTab {
        .init(section: .memory, projectId: id, itemId: "memory", mode: .source, title: "Memory")
    }

    private func client(status: Int) -> ServerClient {
        ServerClient(daemon: DaemonXPCClient(serviceName: "test.unused")) { request in
            if request.method == "DELETE" {
                XCTAssertEqual(request.headers["If-Match"], "1")
                return .init(status: status, headers: [:], body: #"{"deleted":true,"id":"deleted"}"#)
            }
            if request.path.contains("/members") {
                return .init(status: 200, headers: [:], body: #"{"items":[],"page_info":{"has_more":false}}"#)
            }
            return .init(status: 200, headers: [:], body: #"{"project_id":"deleted","name":"Deleted","description":"","member_count":1,"revision":1,"created_at":"now","updated_at":"now"}"#)
        }
    }
}
