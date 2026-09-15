import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class StartupWindowLayoutTests: XCTestCase {
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
