import XCTest
@testable import Clumsies

final class OrganizationAccessLoadingTests: XCTestCase {
    func testAccessLoadsOnlyOrganizationAndSignInSettingsAndRequiresBothToBeFresh() async throws {
        for stalePath: String? in [nil, "/api/v1/admin/org", "/api/v1/admin/identity-provider"] {
            let page = try await AdministrationModel.loadAdministrationPage(section: .access) { path, query in
                XCTAssertTrue(query.isEmpty)
                let body: String
                switch path {
                case "/api/v1/admin/org":
                    body = #"{"org_id":"org","name":"Example","allowed_email_domains":["example.com"],"revision":1,"updated_at":"2026-09-08T10:00:00Z"}"#
                case "/api/v1/admin/identity-provider":
                    body = #"{"protocol":"oidc","configured":true,"admission_mode":"invite_only","secret_source":"environment"}"#
                default:
                    XCTFail("Access settings must not request credentials or another page: \(path)")
                    throw ServerClientError.invalidResponse("Unexpected endpoint")
                }
                return DaemonServerResponse(status: 200,
                    headers: path == stalePath ? ["x-clumsies-cache": "stale"] : [:], body: body)
            }
            XCTAssertEqual(page.snapshot.organization?.allowedEmailDomains, ["example.com"])
            XCTAssertEqual(page.snapshot.identityProvider?.configured, true)
            XCTAssertNil(page.nextCursor)
            XCTAssertEqual(page.isStale, stalePath != nil)
        }
    }
}
