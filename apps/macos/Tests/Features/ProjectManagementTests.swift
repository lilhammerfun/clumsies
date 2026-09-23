import AppKit
import Foundation
import SwiftUI
import XCTest
@testable import Clumsies

final class ProjectManagementTests: XCTestCase {
    @MainActor
    func testCreationSheetFitsItsContentAndCapsRepositoryList() throws {
        let workspace = WorkspaceCoordinator()
        workspace.bundles.bundleLoadState = .loaded
        workspace.bundles.bundles = [.init(
            id: "team", name: "Team Knowledge", description: "", resourceIds: [], revision: 1, updatedAt: "now"
        )]
        for language in ["en", "zh-Hans"] {
            for scheme in [ColorScheme.light, .dark] {
                var heights: [CGFloat] = []
                for count in [0, 1, 3, 8] {
                    let model = ProjectCreationModel(projects: workspace.projects)
                    model.name = "智能投标项目"
                    model.selectedBundleId = "team"
                    model.repositories = (0..<count).map {
                        URL(fileURLWithPath: "/Users/example/workspace/repository-\($0)/client")
                    }
                    let size = try snapshot(ProjectCreationSheet(model: model)
                        .workspaceEnvironment(workspace)
                        .environment(\.locale, Locale(identifier: language)),
                        name: "New Project-\(language)-\(scheme)-\(count) repositories", scheme: scheme)
                    XCTAssertEqual(size.width, 520, accuracy: 1)
                    XCTAssertLessThan(size.height, 560, "Creation should stay compact even with many repositories.")
                    heights.append(size.height)
                }
                XCTAssertGreaterThan(heights[1], heights[0], "Adding a repository must expand the sheet.")
                XCTAssertGreaterThan(heights[2], heights[1])
                XCTAssertEqual(heights[3], heights[2], accuracy: 1, "Additional repositories should scroll, not grow the sheet.")
            }
        }
    }

    @MainActor
    func testSheetActionBarKeepsItsSizeAcrossSubmissionStates() throws {
        for language in ["en", "zh-Hans"] {
            for scheme in [ColorScheme.light, .dark] {
                var sizes: [NSSize] = []
                for state in ["ready", "invalid", "working"] {
                    sizes.append(try snapshot(SheetActionBar(
                        confirmationTitle: Text("Create Project"), progressTitle: "Creating project…",
                        isWorking: state == "working", canConfirm: state != "invalid",
                        cancel: {}, confirm: {}
                    )
                    .frame(width: 520)
                    .environment(\.locale, Locale(identifier: language)),
                    name: "Sheet Actions-\(language)-\(scheme)-\(state)", scheme: scheme))
                }
                XCTAssertEqual(sizes[0], sizes[1], "Disabling the primary action must not move the buttons.")
                XCTAssertEqual(sizes[0], sizes[2], "Progress must not change the action bar's size.")
            }
        }
    }

    @MainActor
    private func snapshot<Content: View>(_ content: Content, name: String, scheme: ColorScheme) throws -> NSSize {
        let host = NSHostingView(rootView: content
            .background(Color(nsColor: .windowBackgroundColor))
            .environment(\.controlActiveState, .key)
            .environment(\.colorScheme, scheme))
        let size = host.fittingSize
        host.frame = NSRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.appearance = NSAppearance(named: scheme == .light ? .aqua : .darkAqua)
        window.contentView = host
        defer { window.close() }
        for _ in 0..<3 {
            host.layoutSubtreeIfNeeded()
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        }
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        let attachment = XCTAttachment(data: try XCTUnwrap(bitmap.representation(using: .png, properties: [:])),
            uniformTypeIdentifier: "public.png")
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        return size
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
        XCTAssertTrue(WorkspaceContext.projectManagementAllowed(capabilities: [], role: .owner))
        XCTAssertTrue(WorkspaceContext.projectManagementAllowed(capabilities: [], role: .admin))
        XCTAssertFalse(WorkspaceContext.projectManagementAllowed(capabilities: ["project:create"], role: .member))
        XCTAssertFalse(WorkspaceContext.projectManagementAllowed(capabilities: ["project:create"], role: nil))
        XCTAssertTrue(WorkspaceContext.projectManagementAllowed(capabilities: ["admin:write"], role: nil))
        XCTAssertFalse(AdministrationModel.administrationMutationAllowed(
            capabilities: ["project:create"], hasSnapshot: true, isStale: false
        ))
    }

    func testProjectReferenceDecodesMembershipRole() throws {
        for role in ProjectMemberRole.allCases {
            let reference = try JSONCoding.decoder().decode(ProjectReference.self, from: Data(
                #"{"project_id":"project-1","name":"My project","role":"\#(role.rawValue)"}"#.utf8
            ))
            XCTAssertEqual(reference.role, role)
        }
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
