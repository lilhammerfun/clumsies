import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class StartupWindowLayoutTests: XCTestCase {
    func testEditingServerKeepsFormAndClearsCredentialsUntilAddressIsChecked() async {
        let model = NativeServerAccessModel(
            serverURL: URL(string: "https://clumsies.example.com")!,
            purpose: .appSignIn, destination: .memoryOnly,
            recoveryState: NativeAdministratorRecoveryState(),
            initialLoginMethods: .init(passwordEnabled: true, oidcEnabled: true, google: true))
        XCTAssertTrue(model.serverReady)
        model.password = "old server password"
        model.confirmPassword = model.password
        model.actionToken = "invitation"
        model.setupCode = "setup secret"
        model.serverOrigin = "invalid address"
        XCTAssertFalse(model.serverReady)
        XCTAssertEqual(model.password, "")
        XCTAssertEqual(model.confirmPassword, "")
        XCTAssertEqual(model.actionToken, "")
        XCTAssertEqual(model.setupCode, "")
        model.signInWithPassword()
        model.continueFromServer()
        model.completeSetup()
        XCTAssertFalse(model.isBusy)
        model.loadLoginMethods()
        while model.isBusy { await Task.yield() }
        XCTAssertNotNil(model.errorMessage)
        XCTAssertEqual(model.loginMethods?.google, true)
        XCTAssertEqual(model.loginMethods?.passwordEnabled, true)
        model.serverOrigin = "https://clumsies.example.com"
        XCTAssertTrue(model.serverReady)
        XCTAssertNil(model.errorMessage)
    }

    func testDualMethodLoginRendersInTheSharedCompactWindow() throws {
        let controller = StartupWindowController()
        defer { controller.close() }
        let model = NativeServerAccessModel(serverURL: URL(string: "https://clumsies.example.com")!,
            purpose: .appSignIn, destination: .memoryOnly,
            recoveryState: NativeAdministratorRecoveryState(),
            initialLoginMethods: .init(passwordEnabled: true, oidcEnabled: true, google: true))
        controller.show(NativeServerAccessView(model: model))
        let content = try XCTUnwrap(controller.window?.contentView)
        content.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        XCTAssertEqual(controller.window?.contentLayoutRect.size, NSSize(width: 540, height: 440))
        XCTAssertNotNil(NSImage(named: "GoogleG"))
        XCTAssertNotNil(NSFont(name: "GoogleSans-Regular_Medium", size: 14))

    }

    func testLocalDevelopmentWaitsForCapabilitiesBeforeOfferingManualSetup() throws {
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

    func testReadyDevelopmentServerAllowsManualPasswordLoginAndSetup() throws {
        let model = NativeServerAccessModel(
            serverURL: URL(string: "http://127.0.0.1:18080")!, purpose: .appSignIn,
            destination: .memoryOnly, recoveryState: NativeAdministratorRecoveryState(),
            initialSetupStatus: .init(state: .setupRequired, setupCodeConfigured: true, oidcConfigured: false, session: nil),
            initialLoginMethods: .init(passwordEnabled: true, oidcEnabled: false, google: false),
            developmentInstanceID: "dev-instance")
        XCTAssertFalse(model.usesAutomaticDevelopmentLogin)
        XCTAssertTrue(model.showsSetup)
    }

    func testSignInInvitationResetAndRecoveryKeepTheStartupWidth() throws {
        let controller = StartupWindowController()
        defer { controller.close() }
        for purpose in [NativeServerAccessModel.Purpose.appSignIn, .administratorRecovery] {
            let model = NativeServerAccessModel(
                purpose: purpose, destination: .memoryOnly,
                recoveryState: NativeAdministratorRecoveryState(),
                initialLoginMethods: .init(passwordEnabled: true, oidcEnabled: true, google: true))
            for action in [NativeServerAccessModel.LocalAction.signIn, .invitation, .reset] {
                model.localAction = action
                controller.show(NativeServerAccessView(model: model))
                controller.window?.contentView?.layoutSubtreeIfNeeded()
                RunLoop.current.run(until: Date().addingTimeInterval(0.1))
                XCTAssertEqual(controller.window?.frame.width, 540)
                XCTAssertEqual(controller.window?.contentLayoutRect.size, StartupWindowController.contentSize)
            }
        }
    }

    func testStartupScreensAndReopeningKeepOneCompactWindow() throws {
        let controller = StartupWindowController()
        defer { controller.close() }
        let form = NativeServerAccessView(model: NativeServerAccessModel(
            purpose: .appSignIn, destination: .memoryOnly,
            recoveryState: NativeAdministratorRecoveryState(),
            initialLoginMethods: .init(passwordEnabled: true, oidcEnabled: true, google: true)
        ))
        let setup = NativeServerAccessView(model: NativeServerAccessModel(
            purpose: .appSignIn, destination: .memoryOnly,
            recoveryState: NativeAdministratorRecoveryState(),
            initialSetupStatus: .init(state: .setupRequired, setupCodeConfigured: true,
                                      oidcConfigured: true, session: nil),
            initialLoginMethods: .init(passwordEnabled: true, oidcEnabled: true, google: true)
        ))
        controller.show(form)
        let window = try XCTUnwrap(controller.window)
        window.setFrameOrigin(NSPoint(x: 150, y: 160))
        let frame = window.frame
        for content in [AnyView(LaunchView()), AnyView(setup),
                        AnyView(FailureView(message: "Server unavailable", retry: {})),
                        AnyView(form)] {
            controller.show(content)
            XCTAssertTrue(controller.window === window)
            XCTAssertEqual(window.frame, frame)
            XCTAssertEqual(window.contentView?.frame.size, NSSize(width: 540, height: 440))
            XCTAssertEqual(window.contentMinSize, NSSize(width: 540, height: 440))
            XCTAssertEqual(window.contentMaxSize, NSSize(width: 540, height: 440))
        }
        window.orderOut(nil)
        controller.show(form)
        XCTAssertTrue(window.isVisible)
        XCTAssertEqual(window.frame, frame)
    }

    func testStartupContentChangesKeepTheSameWindowFrameAndBackground() throws {
        let controller = StartupWindowController()
        defer { controller.close() }
        controller.show(NativeServerAccessView(model: NativeServerAccessModel(
            purpose: .appSignIn, destination: .memoryOnly,
            recoveryState: NativeAdministratorRecoveryState(),
            initialLoginMethods: .init(passwordEnabled: true, oidcEnabled: true, google: true)
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
