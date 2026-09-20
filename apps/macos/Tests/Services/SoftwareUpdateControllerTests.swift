import Combine
import Sparkle
import XCTest
@testable import Clumsies

@MainActor
final class SoftwareUpdateControllerTests: XCTestCase {
    func testPreviewBuildVerifiesInstallableUpdatesBeforeExtraction() {
        XCTAssertTrue((Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") as? String)?.hasSuffix("/preview-appcast.xml") == true)
        XCTAssertNil(Bundle.main.object(forInfoDictionaryKey: "SUAllowsAutomaticUpdates"))
        XCTAssertEqual(Bundle.main.object(forInfoDictionaryKey: "SUVerifyUpdateBeforeExtraction") as? Bool, true)
    }

    func testSparkleChangesReachSettingsWithoutWritingTheInstalledAppsPreferences() async throws {
        let identifier = "ai.clumsies.update-test.\(UUID().uuidString)"
        let directory = FileManager.default.temporaryDirectory.appending(path: "\(identifier).app")
        let contents = directory.appending(path: "Contents")
        let defaults = UserDefaults(suiteName: identifier)!
        defer {
            defaults.removePersistentDomain(forName: identifier)
            try? FileManager.default.removeItem(at: directory)
        }
        try FileManager.default.createDirectory(at: contents, withIntermediateDirectories: true)
        let plist: [String: Any] = [
            "CFBundleIdentifier": identifier,
            "CFBundleName": "Update Test",
            "CFBundleVersion": "1",
            "CFBundleShortVersionString": "0.1.0",
            "SUFeedURL": "https://updates.invalid/appcast.xml",
            "SUEnableAutomaticChecks": false,
            "SUPublicEDKey": "oCAiBe/ez4wochO1I9ziO1uCEpmES+e7ypC74HJwIvw=",
        ]
        try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
            .write(to: contents.appending(path: "Info.plist"))
        let bundle = try XCTUnwrap(Bundle(url: directory))
        let driver = SPUStandardUserDriver(hostBundle: bundle, delegate: nil)
        let updater = SPUUpdater(hostBundle: bundle, applicationBundle: bundle, userDriver: driver, delegate: nil)
        let controller = SoftwareUpdateController(updater: updater)
        XCTAssertFalse(controller.canCheckForUpdates)

        let started = expectation(description: "Settings observes Sparkle becoming ready")
        var observation = controller.objectWillChange
            .filter { controller.canCheckForUpdates }
            .prefix(1)
            .sink { started.fulfill() }
        try updater.start()
        await fulfillment(of: [started], timeout: 2)
        observation.cancel()
        XCTAssertTrue(controller.canCheckForUpdates)

        controller.automaticallyChecksForUpdates = true
        controller.automaticallyDownloadsUpdates = true
        XCTAssertTrue(controller.allowsAutomaticUpdates)
        XCTAssertTrue(defaults.bool(forKey: "SUEnableAutomaticChecks"))
        XCTAssertTrue(defaults.bool(forKey: "SUAutomaticallyUpdate"))
        // Drain the setting changes before testing a change made outside the wrapper.
        let enabled = expectation(description: "Enabled preferences reach Settings")
        observation = controller.objectWillChange.prefix(1).sink { enabled.fulfill() }
        await fulfillment(of: [enabled], timeout: 2)
        observation.cancel()

        let changed = expectation(description: "Sparkle preference changes reach Settings")
        observation = controller.objectWillChange
            .filter { !controller.automaticallyChecksForUpdates && !controller.allowsAutomaticUpdates }
            .prefix(1)
            .sink { changed.fulfill() }
        updater.automaticallyChecksForUpdates = false
        await fulfillment(of: [changed], timeout: 2)
        observation.cancel()
        XCTAssertFalse(controller.automaticallyDownloadsUpdates)
        XCTAssertFalse(defaults.bool(forKey: "SUEnableAutomaticChecks"))
    }
}
