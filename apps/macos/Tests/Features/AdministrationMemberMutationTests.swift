import XCTest
@testable import Clumsies

@MainActor
final class AdministrationMemberMutationTests: XCTestCase {
    func testMemberChangesRefreshProjectAndReloadWorkspaceOnlyForCurrentUser() async throws {
        for (userId, initialRole) in [("other", ProjectMemberRole.admin), ("current", .member)] {
            let context = WorkspaceContext()
            context.phase = .ready
            context.capabilities = ["admin:write"]
            context.account = .init(userId: "current", email: "current@example.com",
                displayName: nil, avatarUrl: nil, role: "admin")
            let responses = MemberMutationResponses(userId: userId)
            context.server = ServerClient(daemon: DaemonXPCClient(serviceName: "test.unused")) {
                try await responses.respond(to: $0)
            }
            var workspaceReloads = 0
            let administration = AdministrationModel(context: context, onWorkspaceChanged: {
                workspaceReloads += 1
            })
            await administration.loadProject(id: "project")
            let picker = ProjectMemberPickerModel(projectId: "project", administration: administration) { _, _ in
                .init(items: [.init(userId: userId, email: "member@example.com",
                    displayName: nil, avatarUrl: nil, role: "member")],
                    pageInfo: .init(nextCursor: nil, hasMore: false))
            }
            await picker.loadMembers()
            picker.selectedId = userId
            XCTAssertEqual(picker.role, .member)
            picker.role = .owner
            XCTAssertFalse(picker.canAdd)
            picker.role = initialRole
            XCTAssertTrue(picker.canAdd)

            let added = await picker.add()
            XCTAssertTrue(added)
            XCTAssertNil(picker.errorMessage)
            XCTAssertEqual(workspaceReloads, userId == "current" ? 1 : 0)
            XCTAssertEqual(administration.projectMembers["project"]?.map(\.id), [userId])
            XCTAssertEqual(administration.projectMembers["project"]?.first?.role, initialRole)
            XCTAssertEqual(administration.project(id: "project")?.memberCount, 1)
            XCTAssertTrue(administration.state(for: .audit).isStale)
            XCTAssertFalse(context.isMutatingAdministration)
            XCTAssertTrue(picker.availableMembers.isEmpty)

            let updatedRole: ProjectMemberRole = initialRole == .member ? .admin : .member
            try await administration.updateAdminProjectMember(projectId: "project", userId: userId, role: updatedRole)
            XCTAssertEqual(administration.projectMembers["project"]?.first?.role, updatedRole)
            XCTAssertEqual(workspaceReloads, userId == "current" ? 2 : 0)

            try await administration.deleteAdminProjectMember(projectId: "project", userId: userId)
            XCTAssertEqual(workspaceReloads, userId == "current" ? 3 : 0)
            XCTAssertEqual(administration.projectMembers["project"], [])
            XCTAssertEqual(administration.project(id: "project")?.memberCount, 0)
            XCTAssertFalse(context.isMutatingAdministration)
            XCTAssertTrue(administration.canMutateProject("project"))
            let mutations = await responses.mutations
            XCTAssertEqual(mutations, ["POST", "PATCH", "DELETE"])
        }
    }

    func testOwnerCannotBeRemovedOrDemotedThroughTheModel() async throws {
        let context = WorkspaceContext()
        context.phase = .ready
        context.capabilities = ["admin:write"]
        let responses = MemberMutationResponses(userId: "owner", role: .owner, hasMember: true)
        context.server = ServerClient(daemon: DaemonXPCClient(serviceName: "test.unused")) {
            try await responses.respond(to: $0)
        }
        let administration = AdministrationModel(context: context, onWorkspaceChanged: {})
        await administration.loadProject(id: "project")
        for role in [ProjectMemberRole.admin, .member] {
            do {
                try await administration.updateAdminProjectMember(projectId: "project", userId: "owner", role: role)
                XCTFail("Owner demotion must fail")
            } catch { XCTAssertTrue(error is ServerClientError) }
        }
        do {
            try await administration.deleteAdminProjectMember(projectId: "project", userId: "owner")
            XCTFail("Owner removal must fail")
        } catch { XCTAssertTrue(error is ServerClientError) }
        let mutations = await responses.mutations
        XCTAssertTrue(mutations.isEmpty)
        XCTAssertEqual(administration.projectMembers["project"]?.first?.role, .owner)
        XCTAssertFalse(context.isMutatingAdministration)
    }
}

private actor MemberMutationResponses {
    let userId: String
    private var hasMember: Bool
    private var role: ProjectMemberRole
    private(set) var mutations: [String] = []

    init(userId: String, role: ProjectMemberRole = .member, hasMember: Bool = false) {
        self.userId = userId
        self.role = role
        self.hasMember = hasMember
    }

    private var member: ProjectMemberRecord {
        ProjectMemberRecord(projectId: "project",
            user: .init(userId: userId, email: "member@example.com",
                displayName: nil, avatarUrl: nil, role: "member"),
            role: role, joinedAt: "now")
    }

    func respond(to request: DaemonServerRequest) throws -> DaemonServerResponse {
        let path = String(request.path.split(separator: "?")[0])
        switch (request.method, path) {
        case ("POST", "/api/v1/admin/projects/project/members"):
            let payload = try JSONCoding.decoder().decode(CreateProjectMemberRequest.self,
                from: Data((request.body ?? "").utf8))
            XCTAssertEqual(payload.userId, userId)
            role = payload.role
            hasMember = true
            mutations.append(request.method)
            return try response(member, status: 201)
        case ("PATCH", "/api/v1/admin/projects/project/members/\(userId)"):
            let payload = try JSONCoding.decoder().decode(UpdateProjectMemberRequest.self,
                from: Data((request.body ?? "").utf8))
            role = payload.role
            mutations.append(request.method)
            return try response(member)
        case ("DELETE", "/api/v1/admin/projects/project/members/\(userId)"):
            hasMember = false
            mutations.append(request.method)
            return DaemonServerResponse(status: 200, headers: [:], body: #"{"deleted":true,"id":"member"}"#)
        case ("GET", "/api/v1/admin/projects/project"):
            return try response(AdminProjectRecord(projectId: "project", name: "Project", description: "",
                memberCount: hasMember ? 1 : 0, revision: 1, createdAt: "now", updatedAt: "now"))
        case ("GET", "/api/v1/admin/projects/project/members"):
            let items = try response(hasMember ? [member] : []).body
            return DaemonServerResponse(status: 200, headers: [:],
                body: #"{"items":\#(items),"page_info":{"next_cursor":null,"has_more":false}}"#)
        default:
            XCTFail("Unexpected request: \(request.method) \(request.path)")
            throw ServerClientError.invalidPath
        }
    }

    private func response<Value: Encodable>(_ value: Value, status: Int = 200) throws -> DaemonServerResponse {
        .init(status: status, headers: [:],
            body: String(decoding: try JSONCoding.encoder().encode(value), as: UTF8.self))
    }
}
