import Foundation
import XCTest

final class TestEnvironmentTests: XCTestCase {
    func testHostedTestsUseTheRequestedIsolatedPreferencesHome() throws {
        guard let expected = ProcessInfo.processInfo.environment["CLUMSIES_ISOLATED_TEST_HOME"] else {
            throw XCTSkip("Run Scripts/test.sh to verify the isolated hosted test environment.")
        }
        XCTAssertEqual(ProcessInfo.processInfo.environment["CFFIXED_USER_HOME"], expected)
        XCTAssertEqual(
            FileManager.default.homeDirectoryForCurrentUser.resolvingSymlinksInPath(),
            URL(fileURLWithPath: expected, isDirectory: true).resolvingSymlinksInPath()
        )
    }
}
