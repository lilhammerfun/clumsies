import Combine
import Network
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
        let bundle = try makeBundle(feedURL: "https://updates.invalid/empty")
        let identifier = try XCTUnwrap(bundle.bundleIdentifier)
        let defaults = UserDefaults(suiteName: identifier)!
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
        XCTAssertFalse(controller.hasAvailableUpdate, "Being ready to check must not advertise an update")

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

    func testSparkleDistinguishesEmptyCurrentNewAndMissingFeeds() async throws {
        let server = try await makeFeedServer()
        defer { server.cancel() }
        let port = try XCTUnwrap(server.port)

        for (path, version, errorCode) in [
            ("empty", nil, Int(SUError.noUpdateError.rawValue)),
            ("current", nil, Int(SUError.noUpdateError.rawValue)),
            ("new", "2", nil),
            ("missing", nil, Int(SUError.downloadError.rawValue)),
        ] as [(String, String?, Int?)] {
            let bundle = try makeBundle(feedURL: "http://127.0.0.1:\(port.rawValue)/\(path)")
            let observer = UpdateCheckObserver(finished: expectation(description: path))
            let driver = SPUStandardUserDriver(hostBundle: bundle, delegate: nil)
            let updater = SPUUpdater(hostBundle: bundle, applicationBundle: bundle, userDriver: driver, delegate: observer)
            let controller = SoftwareUpdateController(updater: updater)
            observer.controller = controller
            var advertisedUpdate = false
            let observation = controller.$hasAvailableUpdate.sink { advertisedUpdate = advertisedUpdate || $0 }

            try updater.start()
            updater.checkForUpdateInformation()
            await fulfillment(of: [observer.finished], timeout: 5)

            XCTAssertEqual(observer.version, version, path)
            XCTAssertEqual(observer.error?.code, errorCode, path)
            XCTAssertFalse(advertisedUpdate, "A version probe cannot present an update: \(path)")
            XCTAssertFalse(controller.hasAvailableUpdate, "Completed probes do not leave stale reminders")
            observation.cancel()
        }
    }

    func testReminderWaitsForAnActionableUpdateInsteadOfBackgroundDownload() async throws {
        let server = try await makeFeedServer()
        defer { server.cancel() }
        let port = try XCTUnwrap(server.port)

        for automaticallyDownloads in [true, false] {
            let bundle = try makeBundle(feedURL: "http://127.0.0.1:\(port.rawValue)/new")
            let observer = UpdateCheckObserver(finished: expectation(description: "Update cycle finishes"))
            let driver = UpdateTestUserDriver(hostBundle: bundle, delegate: observer)
            let updater = SPUUpdater(hostBundle: bundle, applicationBundle: bundle, userDriver: driver, delegate: observer)
            let controller = SoftwareUpdateController(updater: updater)
            observer.controller = controller
            observer.presented = automaticallyDownloads ? nil : expectation(description: "Update can be presented")
            updater.automaticallyChecksForUpdates = true
            updater.automaticallyDownloadsUpdates = automaticallyDownloads
            try updater.start()
            updater.checkForUpdatesInBackground()

            if automaticallyDownloads {
                // The local download returns 404; no user-facing update is ready in this cycle.
                await fulfillment(of: [observer.finished], timeout: 5)
                XCTAssertEqual(observer.reminderWhileDownloading, false)
                XCTAssertEqual(observer.canCheckWhileDownloading, false)
                XCTAssertEqual(observer.error?.code, Int(SUError.downloadError.rawValue))
                XCTAssertFalse(controller.hasAvailableUpdate)
                // Sparkle may briefly become busy again while asynchronously scheduling
                // its next check after the completion delegate returns.
                let retryReady = expectation(description: "Failed download becomes retryable")
                let readiness = updater.publisher(for: \.canCheckForUpdates, options: [.initial, .new])
                    .filter { $0 }
                    .prefix(1)
                    .sink { _ in retryReady.fulfill() }
                await fulfillment(of: [retryReady], timeout: 5)
                readiness.cancel()
                XCTAssertTrue(controller.canCheckForUpdates, "A failed download must allow retrying")
            } else {
                await fulfillment(of: [try XCTUnwrap(observer.presented)], timeout: 5)
                XCTAssertTrue(controller.hasAvailableUpdate)
                XCTAssertTrue(controller.canCheckForUpdates, "The reminder must open an existing update")
                controller.checkForUpdates()
                XCTAssertEqual(driver.focusRequests, 1, "Clicking the reminder must focus the prepared update")
                let reply = try XCTUnwrap(driver.reply)
                driver.reply = nil
                reply(.dismiss)
                await fulfillment(of: [observer.finished], timeout: 5)
                XCTAssertFalse(controller.hasAvailableUpdate, "Dismissing an update must clear the reminder")
            }
            driver.dismissUpdateInstallation()
        }
    }

    func testInstallOnQuitReminderSurvivesCycleCompletionWithoutTakingOverInstallation() throws {
        let bundle = try makeBundle(feedURL: "https://updates.invalid/empty")
        let driver = SPUStandardUserDriver(hostBundle: bundle, delegate: nil)
        let updater = SPUUpdater(hostBundle: bundle, applicationBundle: bundle, userDriver: driver, delegate: nil)
        let controller = SoftwareUpdateController(updater: updater)
        let delegate = controller as SPUUpdaterDelegate
        try updater.start()

        for error in [nil, NSError(domain: SUSparkleErrorDomain, code: Int(SUError.installationError.rawValue))] {
            // Replay SPUAutomaticUpdateDriver's preparation -> install-on-quit -> cycle-ended handoff.
            // No installer is launched and Sparkle must retain ownership of automatic installation.
            let handled = delegate.updater?(updater, willInstallUpdateOnQuit: .empty(), immediateInstallationBlock: {
                XCTFail("The reminder must not force installation or quit the application")
            }) ?? false
            XCTAssertFalse(handled)
            XCTAssertFalse(controller.hasAvailableUpdate, "Wait until the background cycle releases the updater")

            controller.updater(updater, didFinishUpdateCycleFor: .updatesInBackground, error: error)
            XCTAssertEqual(controller.hasAvailableUpdate, error == nil, "A prepared update must remain discoverable until dismissed")
            XCTAssertTrue(controller.canCheckForUpdates)

            controller.standardUserDriverWillFinishUpdateSession()
            XCTAssertFalse(controller.hasAvailableUpdate)
            controller.updater(updater, didFinishUpdateCycleFor: .updatesInBackground, error: nil)
            XCTAssertFalse(controller.hasAvailableUpdate, "An old install-on-quit callback must not revive a dismissed reminder")
        }
    }

    private func makeFeedServer() async throws -> NWListener {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        let listener = try NWListener(using: parameters)
        let ready = expectation(description: "Local update feed server")
        listener.stateUpdateHandler = { if case .ready = $0 { ready.fulfill() } }
        listener.newConnectionHandler = { [weak listener] connection in
            guard let port = listener?.port else { connection.cancel(); return }
            connection.start(queue: .global())
            // All fixture paths fit in the first 16 bytes of the request line.
            connection.receive(minimumIncompleteLength: 16, maximumLength: 8192) { data, _, _, _ in
                guard let data else { connection.cancel(); return }
                let path = String(decoding: data, as: UTF8.self).split(separator: " ").dropFirst().first ?? ""
                let status = path == "/missing" ? "404 Not Found" : "200 OK"
                let version = path == "/new" ? "2" : "1"
                let item = ["/current", "/new"].contains(path) ? """
                    <item><sparkle:version>\(version)</sparkle:version>
                    <enclosure url="http://127.0.0.1:\(port.rawValue)/missing" length="1" type="application/octet-stream" /></item>
                    """ : ""
                let xml = """
                    <rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
                    <channel><title>Clumsies macOS Updates</title>\(item)</channel></rss>
                    """
                let response = "HTTP/1.1 \(status)\r\nContent-Type: application/xml\r\nContent-Length: \(xml.utf8.count)\r\nConnection: close\r\n\r\n\(xml)"
                connection.send(content: Data(response.utf8), completion: .contentProcessed { _ in connection.cancel() })
            }
        }
        listener.start(queue: .global())
        await fulfillment(of: [ready], timeout: 2)
        return listener
    }

    private func makeBundle(feedURL: String) throws -> Bundle {
        let identifier = "ai.clumsies.update-test.\(UUID().uuidString)"
        let directory = FileManager.default.temporaryDirectory.appending(path: "\(identifier).app")
        let contents = directory.appending(path: "Contents")
        addTeardownBlock {
            UserDefaults(suiteName: identifier)?.removePersistentDomain(forName: identifier)
            try FileManager.default.removeItem(at: directory)
        }
        try FileManager.default.createDirectory(at: contents, withIntermediateDirectories: true)
        let plist: [String: Any] = [
            "CFBundleIdentifier": identifier,
            "CFBundleName": "Update Test",
            "CFBundleVersion": "1",
            "CFBundleShortVersionString": "0.1.0",
            "SUFeedURL": feedURL,
            "SUEnableAutomaticChecks": false,
            "SUVerifyUpdateBeforeExtraction": true,
            "SUPublicEDKey": "oCAiBe/ez4wochO1I9ziO1uCEpmES+e7ypC74HJwIvw=",
        ]
        try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
            .write(to: contents.appending(path: "Info.plist"))
        return try XCTUnwrap(Bundle(url: directory))
    }
}

@MainActor
private final class UpdateCheckObserver: NSObject, SPUUpdaterDelegate, @preconcurrency SPUStandardUserDriverDelegate {
    let finished: XCTestExpectation
    var controller: SoftwareUpdateController?
    var version: String?
    var error: NSError?
    var presented: XCTestExpectation?
    var reminderWhileDownloading: Bool?
    var canCheckWhileDownloading: Bool?

    var supportsGentleScheduledUpdateReminders: Bool { true }

    func standardUserDriverShouldHandleShowingScheduledUpdate(_ update: SUAppcastItem, andInImmediateFocus immediateFocus: Bool) -> Bool {
        false
    }

    func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool, forUpdate update: SUAppcastItem, state: SPUUserUpdateState) {
        controller?.standardUserDriverWillHandleShowingUpdate(handleShowingUpdate, forUpdate: update, state: state)
        presented?.fulfill()
    }

    func standardUserDriverWillFinishUpdateSession() {
        controller?.standardUserDriverWillFinishUpdateSession()
    }

    func updater(_ updater: SPUUpdater, willDownloadUpdate item: SUAppcastItem, with request: NSMutableURLRequest) {
        reminderWhileDownloading = controller?.hasAvailableUpdate
        canCheckWhileDownloading = controller?.canCheckForUpdates
    }

    init(finished: XCTestExpectation) { self.finished = finished }

    func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        version = item.versionString
        if let controller {
            (controller as SPUUpdaterDelegate).updater?(updater, didFindValidUpdate: item)
        }
    }

    func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck, error: Error?) {
        self.error = error as NSError?
        controller?.updater(updater, didFinishUpdateCycleFor: updateCheck, error: error)
        finished.fulfill()
    }
}

@MainActor
private final class UpdateTestUserDriver: SPUStandardUserDriver {
    var reply: ((SPUUserUpdateChoice) -> Void)?
    var focusRequests = 0

    override func showUpdateInFocus() {
        focusRequests += 1
    }

    override func showUpdateFound(with appcastItem: SUAppcastItem, state: SPUUserUpdateState, reply: @escaping (SPUUserUpdateChoice) -> Void) {
        self.reply = reply
        super.showUpdateFound(with: appcastItem, state: state, reply: reply)
    }
}
