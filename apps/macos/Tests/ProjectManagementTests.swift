import Foundation
import XCTest
@testable import Clumsies

final class ProjectManagementTests: XCTestCase {
    func testRepositoryIntegrationsExcludeGlobalCodexPlugin() {
        XCTAssertEqual(
            ProjectAgentAdapterKind.repositoryIntegrationCases,
            [.claudeCode, .opencode, .dsh, .antigravity]
        )
    }

    func testAgentConfigurationLivesInGlobalSettings() throws {
        let macOSRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let settings = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/SettingsView.swift"),
            encoding: .utf8
        )
        let projectManagement = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/ProjectManagementView.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(settings.contains("Section(\"Repository Integrations\")"))
        XCTAssertTrue(settings.contains("ProjectAgentAdapterKind.repositoryIntegrationCases"))
        XCTAssertGreaterThanOrEqual(
            settings.components(separatedBy: "try Task.checkCancellation()").count - 1,
            2
        )
        XCTAssertFalse(projectManagement.contains("Section(\"Agent\")"))
    }

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
        let filter = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Components/ToolbarFilterMenu.swift"),
            encoding: .utf8
        )
        let workspace = try String(
            contentsOf: macOSRoot.appending(path: "Sources/Features/WorkspaceView.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(filter.contains("Button(\"New Project…\", systemImage: \"plus\")"))
        XCTAssertEqual(
            workspace.components(separatedBy: "onCreate: store.canManageProjects").count - 1,
            2
        )
        XCTAssertEqual(
            workspace.components(separatedBy: "ProjectCreationSheet(store: store)").count - 1,
            1
        )
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
