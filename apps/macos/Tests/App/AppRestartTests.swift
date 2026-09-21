import Foundation
import XCTest
@testable import Clumsies

@MainActor
final class AppRestartTests: XCTestCase {
    func testCancelledQuitOrFailedSaveDoesNotArmRelaunchOrAffectLaterQuit() throws {
        var launches = 0
        let controller = AppRestartController {
            launches += 1
            return .nullDevice
        }
        var terminationRequests = 0
        controller.request { terminationRequests += 1 }
        controller.request { terminationRequests += 1 }
        XCTAssertEqual(terminationRequests, 1)
        XCTAssertEqual(launches, 0, "Requesting a restart must wait for the quit/save guards.")
        XCTAssertFalse(try controller.finishTermination(allowed: false))
        XCTAssertTrue(try controller.finishTermination(allowed: true))
        XCTAssertEqual(launches, 0, "A later ordinary quit must not restart the App.")

        controller.request { terminationRequests += 1 }
        XCTAssertTrue(try controller.finishTermination(allowed: true))
        XCTAssertTrue(try controller.finishTermination(allowed: true))
        XCTAssertEqual(launches, 1)
        XCTAssertEqual(terminationRequests, 2)
    }

    func testHelperLaunchFailureRejectsTerminationAndAllowsExplicitRetry() throws {
        var shouldFail = true
        var attempts = 0
        let controller = AppRestartController {
            attempts += 1
            if shouldFail { throw CocoaError(.executableNotLoadable) }
            return .nullDevice
        }
        controller.request {}
        XCTAssertThrowsError(try controller.finishTermination(allowed: true))
        shouldFail = false
        XCTAssertTrue(try controller.finishTermination(allowed: true))
        XCTAssertEqual(attempts, 1, "A failed restart must not turn an ordinary quit into a retry.")
        controller.request {}
        XCTAssertTrue(try controller.finishTermination(allowed: true))
        XCTAssertEqual(attempts, 2)
    }

    func testRelaunchWaitsForExitAndPassesTheExactAppPathAsAnArgument() async throws {
        let directory = FileManager.default.temporaryDirectory.appending(path: "AppRestartTests-\(UUID())")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let opener = directory.appending(path: "test open")
        let recorded = directory.appending(path: "test open.args")
        try "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\n".write(to: opener, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: opener.path)
        let app = directory.appending(path: "中文 App's $(echo wrong).app")
        let exitSignal = try AppRestartController.prepare(applicationURL: app, openCommand: opener)
        defer { try? exitSignal.close() }

        let exists = NSPredicate { _, _ in FileManager.default.fileExists(atPath: recorded.path) }
        let earlyLaunch = XCTNSPredicateExpectation(predicate: exists, object: nil)
        earlyLaunch.isInverted = true
        await fulfillment(of: [earlyLaunch], timeout: 0.2)
        // Closing this descriptor models the OS closing it at App process exit.
        try exitSignal.close()
        let launch = XCTNSPredicateExpectation(predicate: exists, object: nil)
        await fulfillment(of: [launch], timeout: 3)
        XCTAssertEqual(try String(contentsOf: recorded, encoding: .utf8), "-n\n\(app.path)\n")
    }
}
