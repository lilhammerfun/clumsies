import XCTest
@testable import Clumsies

@MainActor
final class AdministrationMemberMutationTests: XCTestCase {
    func testMemberChangesRefreshProjectAndReloadWorkspaceOnlyForCurrentUser() async throws {
        for userId in ["other", "current"] {
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
            XCTAssertTrue(picker.canAdd)

            let added = await picker.add()
            XCTAssertTrue(added)
            XCTAssertNil(picker.errorMessage)
            XCTAssertEqual(workspaceReloads, userId == "current" ? 1 : 0)
            XCTAssertEqual(administration.projectMembers["project"]?.map(\.id), [userId])
            XCTAssertEqual(administration.project(id: "project")?.memberCount, 1)
            XCTAssertTrue(administration.state(for: .audit).isStale)
            XCTAssertFalse(context.isMutatingAdministration)
            XCTAssertTrue(picker.availableMembers.isEmpty)

            try await administration.deleteAdminProjectMember(projectId: "project", userId: userId)
            XCTAssertEqual(workspaceReloads, userId == "current" ? 2 : 0)
            XCTAssertEqual(administration.projectMembers["project"], [])
            XCTAssertEqual(administration.project(id: "project")?.memberCount, 0)
            XCTAssertFalse(context.isMutatingAdministration)
            XCTAssertTrue(administration.canMutateProject("project"))
            let mutations = await responses.mutations
            XCTAssertEqual(mutations, ["POST", "DELETE"])
        }
    }
}

private actor MemberMutationResponses {
    let userId: String
    private var hasMember = false
    private(set) var mutations: [String] = []

    init(userId: String) { self.userId = userId }

    func respond(to request: DaemonServerRequest) throws -> DaemonServerResponse {
        let path = String(request.path.split(separator: "?")[0])
        let member = ProjectMemberRecord(projectId: "project",
            user: .init(userId: userId, email: "member@example.com",
                displayName: nil, avatarUrl: nil, role: "member"),
            role: .member, joinedAt: "now")
        switch (request.method, path) {
        case ("POST", "/api/v1/admin/projects/project/members"):
            let payload = try JSONCoding.decoder().decode(CreateProjectMemberRequest.self,
                from: Data((request.body ?? "").utf8))
            XCTAssertEqual(payload.userId, userId)
            XCTAssertEqual(payload.role, .member)
            hasMember = true
            mutations.append(request.method)
            return try response(member, status: 201)
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
