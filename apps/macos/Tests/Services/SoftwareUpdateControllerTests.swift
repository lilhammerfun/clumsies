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

        var availability: [Bool] = []
        let availabilityObservation = controller.$hasAvailableUpdate.sink { availability.append($0) }
        // Sparkle owns version comparison; exercise its availability and session callbacks.
        let item = SUAppcastItem.empty()
        controller.updater(updater, didFindValidUpdate: item)
        XCTAssertTrue(controller.hasAvailableUpdate)
        controller.standardUserDriverWillFinishUpdateSession()
        XCTAssertFalse(controller.hasAvailableUpdate, "Dismissed or skipped updates must clear the reminder")
        controller.updater(updater, didFindValidUpdate: item)
        controller.updater(updater, didFinishUpdateCycleFor: .updatesInBackground, error: NSError(domain: NSURLErrorDomain, code: NSURLErrorBadServerResponse))
        XCTAssertFalse(controller.hasAvailableUpdate, "A failed update check must not leave a stale reminder")
        controller.updater(updater, didFindValidUpdate: item)
        controller.updater(updater, didFinishUpdateCycleFor: .updatesInBackground, error: nil)
        XCTAssertFalse(controller.hasAvailableUpdate)
        XCTAssertEqual(availability, [false, true, false, true, false, true, false])
        availabilityObservation.cancel()

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
            XCTAssertEqual(advertisedUpdate, version != nil, path)
            XCTAssertFalse(controller.hasAvailableUpdate, "Completed probes do not leave stale reminders")
            observation.cancel()
        }
    }

    private func makeFeedServer() async throws -> NWListener {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        let listener = try NWListener(using: parameters)
        let ready = expectation(description: "Local update feed server")
        listener.stateUpdateHandler = { if case .ready = $0 { ready.fulfill() } }
        listener.newConnectionHandler = { connection in
            connection.start(queue: .global())
            // All fixture paths fit in the first 16 bytes of the request line.
            connection.receive(minimumIncompleteLength: 16, maximumLength: 8192) { data, _, _, _ in
                guard let data else { connection.cancel(); return }
                let path = String(decoding: data, as: UTF8.self).split(separator: " ").dropFirst().first ?? ""
                let status = path == "/missing" ? "404 Not Found" : "200 OK"
                let version = path == "/new" ? "2" : "1"
                let item = ["/current", "/new"].contains(path) ? """
                    <item><sparkle:version>\(version)</sparkle:version>
                    <enclosure url="https://updates.invalid/Clumsies.zip" length="1" type="application/octet-stream" /></item>
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
private final class UpdateCheckObserver: NSObject, SPUUpdaterDelegate {
    let finished: XCTestExpectation
    var controller: SoftwareUpdateController?
    var version: String?
    var error: NSError?

    init(finished: XCTestExpectation) { self.finished = finished }

    func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        version = item.versionString
        controller?.updater(updater, didFindValidUpdate: item)
    }

    func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck, error: Error?) {
        self.error = error as NSError?
        controller?.updater(updater, didFinishUpdateCycleFor: updateCheck, error: error)
        finished.fulfill()
    }
}
