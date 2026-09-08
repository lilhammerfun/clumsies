import XCTest
@testable import Clumsies

final class AdministrationLoadingTests: XCTestCase {
    func testEachPageRequestsOnlyItsOwnResourcesAndStopsAtOnePage() async throws {
        for (section, expectedPaths) in [
            (AdministrationSection.organization, ["/api/v1/admin/org"]),
            (.members, ["/api/v1/admin/members"]),
            (.projects, ["/api/v1/admin/projects"]),
            (.access, ["/api/v1/admin/org", "/api/v1/admin/identity-provider"]),
            (.audit, ["/api/v1/admin/audit-events"]),
        ] {
            let requests = AdministrationRequests()
            let page = try await WorkspaceStore.loadAdministrationPage(section: section) { path, query in
                await requests.record(path: path, query: query)
                return Self.response(path: path, nextCursor: "next-page")
            }
            let paths = await requests.paths
            XCTAssertEqual(paths, expectedPaths)
            XCTAssertEqual(page.nextCursor, (section == .organization || section == .access) ? nil : "next-page")
            XCTAssertFalse(page.isStale)
        }
    }

    func testProjectHistoryLoadsOneProjectByIDWithoutLoadingTheDirectory() async throws {
        let requests = AdministrationRequests()
        let expected = Self.project(id: "off-page")
        let result = try await WorkspaceStore.fetchAdministrationProject(id: expected.id) { path in
            await requests.record(path: path, query: [])
            return DaemonServerResponse(status: 200, headers: ["x-clumsies-cache": "stale"],
                body: String(decoding: try JSONCoding.encoder().encode(expected), as: UTF8.self))
        }
        let paths = await requests.paths
        XCTAssertEqual(paths, ["/api/v1/admin/projects/off-page"])
        XCTAssertEqual(result.project, expected)
        XCTAssertTrue(result.isStale)
    }

    func testProjectHistoryReportsRequestFailureAndRejectsCancelledResponses() async throws {
        for status in [404, 500] {
            do {
                _ = try await WorkspaceStore.fetchAdministrationProject(id: "off-page") { _ in
                    DaemonServerResponse(status: status, headers: [:], body: "Request failed")
                }
                XCTFail("A failed project request must not produce a project or imply successful deletion")
            } catch let error as ServerClientError {
                guard case .response(let actual, _) = error else { return XCTFail("Unexpected error: \(error)") }
                XCTAssertEqual(actual, status)
            }
        }
        let task = Task.detached {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await WorkspaceStore.fetchAdministrationProject(id: "off-page") { _ in
                DaemonServerResponse(status: 200, headers: [:],
                    body: String(decoding: try JSONCoding.encoder().encode(AdministrationLoadingTests.project(id: "off-page")), as: UTF8.self))
            }
        }
        do {
            _ = try await task.value
            XCTFail("A cancelled response must not reach the project cache")
        } catch is CancellationError {}
    }

    func testSearchQueryPersistsAcrossPagesAndEmptyQueryIsOmitted() async throws {
        for section: AdministrationSection in [.members, .audit] {
            let requests = AdministrationRequests()
            for cursor: String? in [nil, "cursor & 2"] {
                _ = try await WorkspaceStore.loadAdministrationPage(
                    section: section, cursor: cursor, query: "  Ada & team  "
                ) { path, query in
                    await requests.record(path: path, query: query)
                    return Self.response(path: path)
                }
            }
            _ = try await WorkspaceStore.loadAdministrationPage(section: section, query: "  ") { path, query in
                await requests.record(path: path, query: query)
                return Self.response(path: path)
            }
            let queries = await requests.queries
            XCTAssertEqual(queries, [
                ["limit": "100", "q": "Ada & team"],
                ["limit": "100", "q": "Ada & team", "cursor": "cursor & 2"],
                ["limit": "100"],
            ])
        }
    }

    func testStaleIdentityProviderMakesAccessReadOnlyWithoutAffectingOrganization() async throws {
        let access = try await WorkspaceStore.loadAdministrationPage(section: .access) { path, _ in
            Self.response(path: path, stale: path.hasSuffix("identity-provider"))
        }
        let organization = try await WorkspaceStore.loadAdministrationPage(section: .organization) { path, _ in
            Self.response(path: path)
        }
        XCTAssertFalse(WorkspaceStore.administrationMutationAllowed(
            capabilities: ["admin:write"], hasSnapshot: true, isStale: access.isStale
        ))
        XCTAssertTrue(WorkspaceStore.administrationMutationAllowed(
            capabilities: ["admin:write"], hasSnapshot: true, isStale: organization.isStale
        ))
        XCTAssertFalse(WorkspaceStore.administrationMutationAllowed(
            capabilities: [], hasSnapshot: true, isStale: false
        ))
    }

    func testOldCapabilitiesNeverPermitWritesOutsideReadyPhase() {
        for phase: ApplicationPhase in [.launching, .loading, .authenticationRequired, .failed("Offline")] {
            XCTAssertFalse(WorkspaceStore.administrationMutationAllowed(
                capabilities: ["admin:write"], phase: phase, hasSnapshot: true, isStale: false
            ))
        }
    }

    func testUpdatingAProjectBeyondTheFirstPagePreservesLoadedProjects() {
        var snapshot = AdministrationSnapshot()
        snapshot.projects = (0..<150).map { Self.project(id: String($0)) }
        snapshot.updateProject(Self.project(id: "149", name: "Renamed", revision: 2))
        XCTAssertEqual(snapshot.projects.count, 150)
        XCTAssertEqual(snapshot.projects.last?.name, "Renamed")
        XCTAssertEqual(snapshot.projects.last?.revision, 2)
        snapshot.updateProject(Self.project(id: "new"))
        XCTAssertEqual(snapshot.projects.count, 151)
        XCTAssertEqual(snapshot.projects.first?.id, "new")
        XCTAssertEqual(snapshot.projects.last?.id, "149")
    }

    func testProjectCursorTracksLocalInsertAndDeleteWithoutSkippingTheNextRecord() {
        var state = AdministrationPageState(
            isLoaded: true, isStale: false, nextCursor: "200", seenCursors: ["100"]
        )
        state.offsetProjectCursor(by: 1)
        XCTAssertEqual(state.nextCursor, "201")
        XCTAssertTrue(state.seenCursors.isEmpty)
        state.offsetProjectCursor(by: -1)
        XCTAssertEqual(state.nextCursor, "200")
        state.offsetProjectCursor(by: -1)
        XCTAssertEqual(state.nextCursor, "199")
        XCTAssertFalse(state.isStale)
        for cursor in ["invalid", String(Int.max)] {
            state.nextCursor = cursor
            state.offsetProjectCursor(by: 1)
            XCTAssertTrue(state.isStale)
            XCTAssertNil(state.nextCursor)
        }
    }

    private static func project(id: String, name: String = "Project", revision: Int = 1) -> AdminProjectRecord {
        AdminProjectRecord(
            projectId: id, name: name, description: "", memberCount: 0, revision: revision,
            createdAt: "2026-09-08T10:00:00Z", updatedAt: "2026-09-08T10:00:00Z"
        )
    }

    func testInvalidPaginationAndFailedRequestsDoNotProduceAPage() async throws {
        for nextCursor in ["", "same", "earlier"] {
            do {
                _ = try await WorkspaceStore.loadAdministrationPage(section: .audit, cursor: "same", seenCursors: ["earlier"]) { path, _ in
                    Self.response(path: path, nextCursor: nextCursor)
                }
                XCTFail("Expected an invalid cursor to fail")
            } catch let error as ServerClientError {
                guard case .invalidResponse = error else { return XCTFail("Unexpected error: \(error)") }
            }
        }
        do {
            _ = try await WorkspaceStore.loadAdministrationPage(section: .members) { _, _ in
                DaemonServerResponse(status: 403, headers: [:], body: "Forbidden")
            }
            XCTFail("Expected permission failure")
        } catch let error as ServerClientError {
            guard case .response(status: 403, _) = error else { return XCTFail("Unexpected error: \(error)") }
        }
    }

    func testCancelledSearchDiscardsALateResponseAfterANewQueryCompletes() async throws {
        let gate = AdministrationResponseGate()
        let oldSearch = Task {
            try await WorkspaceStore.loadAdministrationPage(section: .members, query: "old") { path, _ in
                await gate.waitForRelease()
                return AdministrationLoadingTests.response(path: path, memberId: "old")
            }
        }
        await gate.waitUntilRequested()
        oldSearch.cancel()
        let current = try await WorkspaceStore.loadAdministrationPage(section: .members, query: "new") { path, _ in
            Self.response(path: path, memberId: "new")
        }
        XCTAssertEqual(current.snapshot.members.map(\.id), ["new"])
        await gate.release()
        do {
            _ = try await oldSearch.value
            XCTFail("An obsolete search response must not reach the snapshot merge")
        } catch is CancellationError {}
    }

    func testApplyingMemberPagesPreservesOtherPagesAndAReplacementDropsPreviousResults() async throws {
        let access = try await WorkspaceStore.loadAdministrationPage(section: .access) { path, _ in
            Self.response(path: path)
        }
        let first = try await WorkspaceStore.loadAdministrationPage(section: .members) { path, _ in
            Self.response(path: path, memberId: "first")
        }
        let second = try await WorkspaceStore.loadAdministrationPage(section: .members, cursor: "next") { path, _ in
            Self.response(path: path, memberId: "second")
        }
        var snapshot = access.snapshot
        snapshot.apply(first.snapshot, section: .members, appending: false)
        snapshot.apply(second.snapshot, section: .members, appending: true)
        snapshot.apply(second.snapshot, section: .members, appending: true)
        XCTAssertEqual(snapshot.organization?.name, "Example")
        XCTAssertNotNil(snapshot.identityProvider)
        XCTAssertEqual(snapshot.members.map(\.id), ["first", "second"])
        snapshot.apply(second.snapshot, section: .members, appending: false)
        XCTAssertEqual(snapshot.members.map(\.id), ["second"])
        XCTAssertEqual(snapshot.organization?.name, "Example")
    }

    func testAuditTargetNamesDecodeAndLegacyPayloadsRemainReadable() throws {
        let legacy = #"{"event_id":"event","actor_user_id":"user","action":"member.updated","target_type":"member","target_id":"user","created_at":"2026-09-08T10:00:00Z"}"#
        let audit = try JSONCoding.decoder().decode(AdminAuditEventRecord.self, from: Data(legacy.utf8))
        XCTAssertNil(audit.actorDisplayName)
        XCTAssertNil(audit.actorEmail)
        XCTAssertNil(audit.targetDisplayName)
        var named = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(legacy.utf8)) as? [String: Any])
        named["target_display_name"] = "Ada"
        let enriched = try JSONCoding.decoder().decode(AdminAuditEventRecord.self,
            from: JSONSerialization.data(withJSONObject: named))
        XCTAssertEqual(enriched.targetDisplayName, "Ada")
    }

    private static func response(
        path: String,
        nextCursor: String? = nil,
        stale: Bool = false,
        memberId: String? = nil
    ) -> DaemonServerResponse {
        let body: String
        switch path {
        case "/api/v1/admin/org":
            body = #"{"org_id":"org","name":"Example","allowed_email_domains":[],"revision":1,"updated_at":"2026-09-08T10:00:00Z"}"#
        case "/api/v1/admin/identity-provider":
            body = #"{"protocol":"oidc","configured":true,"admission_mode":"invite_only","secret_source":"environment"}"#
        default:
            let items = memberId.map {
                #"[{"user_id":"\#($0)","email":"ada@example.com","role":"member","status":"active","external_identity_bound":true,"revision":1}]"#
            } ?? "[]"
            let cursor = nextCursor.map { "\"\($0)\"" } ?? "null"
            body = #"{"items":\#(items),"page_info":{"next_cursor":\#(cursor),"has_more":\#(nextCursor != nil)}}"#
        }
        return DaemonServerResponse(
            status: 200,
            headers: stale ? ["x-clumsies-cache": "stale"] : [:],
            body: body
        )
    }
}

private actor AdministrationRequests {
    var paths: [String] = []
    var queries: [[String: String]] = []

    func record(path: String, query: [URLQueryItem]) {
        paths.append(path)
        queries.append(Dictionary(uniqueKeysWithValues: query.map { ($0.name, $0.value ?? "") }))
    }
}

private actor AdministrationResponseGate {
    private var response: CheckedContinuation<Void, Never>?
    private var requested: CheckedContinuation<Void, Never>?

    func waitForRelease() async {
        await withCheckedContinuation { continuation in
            response = continuation
            requested?.resume()
            requested = nil
        }
    }

    func waitUntilRequested() async {
        if response != nil { return }
        await withCheckedContinuation { requested = $0 }
    }

    func release() {
        response?.resume()
        response = nil
    }
}
