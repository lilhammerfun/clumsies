import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class StartupWindowLayoutTests: XCTestCase {
    func testLocalDevelopmentNeverPresentsManualSetupButPrivateServersStillDo() throws {
        let status = NativeSetupStatus(state: .setupRequired, setupCodeConfigured: true,
                                       oidcConfigured: true, session: nil)
        for (origin, instance, automatic) in [
            ("http://127.0.0.1:18080", "dev-instance" as String?, true),
            ("http://localhost:18080", "dev-instance", true),
            ("http://127.0.0.1:18080", nil, false),
            ("https://private.example.com", nil, false),
            ("https://preview.example.com", "dev-instance", false),
        ] {
            let model = NativeServerAccessModel(
                serverURL: try XCTUnwrap(URL(string: origin)), purpose: .appSignIn,
                destination: .memoryOnly, recoveryState: NativeAdministratorRecoveryState(),
                initialSetupStatus: status, developmentInstanceID: instance
            )
            XCTAssertEqual(model.usesAutomaticDevelopmentLogin, automatic)
            XCTAssertEqual(model.showsSetup, !automatic)
        }
    }

    func testCompactLoadingResizesTheSameWindowAroundItsCenter() throws {
        let controller = StartupWindowController()
        defer { controller.close() }
        let form = NativeServerAccessView(model: NativeServerAccessModel(
            purpose: .appSignIn, destination: .memoryOnly,
            recoveryState: NativeAdministratorRecoveryState()
        ))
        // Let AppKit place the tallest content before checking centered resizing.
        controller.show(form)
        let window = try XCTUnwrap(controller.window)
        let formFrame = window.frame
        controller.show(ProgressView("Connecting…"), height: 360)
        let compactFrame = window.frame
        XCTAssertEqual(window.contentView?.frame.size, NSSize(width: 540, height: 360))
        XCTAssertEqual(compactFrame.midX, formFrame.midX)
        XCTAssertEqual(compactFrame.midY, formFrame.midY)

        controller.show(form)
        XCTAssertTrue(controller.window === window)
        XCTAssertEqual(window.contentView?.frame.size, StartupWindowController.contentSize)
        XCTAssertEqual(window.frame, formFrame)
        XCTAssertEqual(window.frame.midX, compactFrame.midX)
        XCTAssertEqual(window.frame.midY, compactFrame.midY)

        controller.show(ProgressView("Syncing team memory…"), height: 360)
        XCTAssertEqual(window.frame, compactFrame)
        XCTAssertEqual(window.contentMinSize, NSSize(width: 540, height: 360))
        XCTAssertEqual(window.contentMaxSize, NSSize(width: 540, height: 360))
    }

    func testStartupContentChangesKeepTheSameWindowFrameAndBackground() throws {
        let controller = StartupWindowController()
        defer { controller.close() }
        controller.show(NativeServerAccessView(model: NativeServerAccessModel(
            purpose: .appSignIn, destination: .memoryOnly,
            recoveryState: NativeAdministratorRecoveryState()
        )))
        let window = try XCTUnwrap(controller.window)
        window.setFrameOrigin(NSPoint(x: 150, y: 160))
        let frame = window.frame
        controller.show(ProgressView("Connecting…"))
        XCTAssertTrue(controller.window === window)
        XCTAssertEqual(window.frame, frame)
        controller.show(VStack { Text("Choose agents"); Text("Codex") })
        XCTAssertEqual(window.frame, frame)
        XCTAssertEqual(window.contentMinSize, StartupWindowController.contentSize)
        XCTAssertEqual(window.contentMaxSize, StartupWindowController.contentSize)
        XCTAssertEqual(window.backgroundColor, NSColor.textBackgroundColor)
        XCTAssertFalse(window.styleMask.contains(.resizable))
        XCTAssertTrue(window.titlebarAppearsTransparent)
    }
}
