import Foundation
import XCTest
@testable import Clumsies

final class ProjectManagementTests: XCTestCase {
    func testProjectMetadataRequiresANonEmptyName() {
        XCTAssertFalse(ProjectMetadataValidation.isValid(name: "   ", description: "Description"))
        XCTAssertTrue(ProjectMetadataValidation.isValid(name: " Project ", description: "Description"))
    }

    func testProjectMetadataEnforcesServerLimits() {
        XCTAssertTrue(
            ProjectMetadataValidation.isValid(
                name: String(repeating: "a", count: 120),
                description: String(repeating: "b", count: 4_000)
            )
        )
        XCTAssertFalse(
            ProjectMetadataValidation.isValid(
                name: String(repeating: "a", count: 121),
                description: ""
            )
        )
        XCTAssertFalse(
            ProjectMetadataValidation.isValid(
                name: "Project",
                description: String(repeating: "b", count: 4_001)
            )
        )
        XCTAssertTrue(
            ProjectMetadataValidation.isValid(
                name: "Project",
                description: " \(String(repeating: "b", count: 4_000)) "
            )
        )
    }

    func testProjectCreationIsAvailableFromEveryProjectFilter() throws {
        let macOSRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let filter = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Libraries/UI/ToolbarFilterMenu.swift"),
            encoding: .utf8
        )
        let workspace = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/Workspace/WorkspaceView.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(filter.contains("Button(\"New Project…\", systemImage: \"plus\")"))
        XCTAssertFalse(filter.contains("All Organization Projects"))
        XCTAssertFalse(workspace.contains("OrganizationProjectsView"))
        XCTAssertEqual(
            workspace.components(separatedBy: "onCreate: workspaceContext.canCreateProject").count - 1,
            2
        )
        XCTAssertEqual(
            workspace.components(separatedBy: "ProjectCreationSheet(model: ProjectCreationModel(projects: store.projects))").count - 1,
            1
        )
    }

    func testProjectCreationAcceptsMemberAndExistingAdministratorCapabilities() {
        XCTAssertTrue(WorkspaceContext.projectCreationAllowed(capabilities: ["project:create"]))
        XCTAssertTrue(WorkspaceContext.projectCreationAllowed(capabilities: ["admin:write"]))
        XCTAssertTrue(WorkspaceContext.projectCreationAllowed(capabilities: ["project:create", "admin:write"]))
        XCTAssertFalse(WorkspaceContext.projectCreationAllowed(capabilities: []))
        XCTAssertFalse(WorkspaceContext.projectCreationAllowed(capabilities: ["memory:read"]))
    }

    func testProjectManagementUsesProjectRoleWithoutGrantingOrganizationAuthority() {
        XCTAssertTrue(WorkspaceContext.projectManagementAllowed(capabilities: [], role: .admin))
        XCTAssertFalse(WorkspaceContext.projectManagementAllowed(capabilities: ["project:create"], role: .member))
        XCTAssertFalse(WorkspaceContext.projectManagementAllowed(capabilities: ["project:create"], role: nil))
        XCTAssertTrue(WorkspaceContext.projectManagementAllowed(capabilities: ["admin:write"], role: nil))
        XCTAssertFalse(AdministrationModel.administrationMutationAllowed(
            capabilities: ["project:create"], hasSnapshot: true, isStale: false
        ))
    }

    func testProjectReferenceDecodesMembershipRole() throws {
        let reference = try JSONCoding.decoder().decode(ProjectReference.self, from: Data(
            #"{"project_id":"project-1","name":"My project","role":"admin"}"#.utf8
        ))
        XCTAssertEqual(reference.role, .admin)
    }

    func testProjectCreationKeepsLocalSetupOutOfTheServerRequest() throws {
        let request = CreateProjectRequest(name: "Server-only Project", description: nil)
        let data = try JSONCoding.encoder().encode(request)
        let json = try XCTUnwrap(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )

        XCTAssertEqual(json["name"] as? String, "Server-only Project")
        XCTAssertEqual(Set(json.keys), ["name"])
        XCTAssertNil(json["repository_paths"])
    }
}
