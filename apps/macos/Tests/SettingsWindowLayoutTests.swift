import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class SettingsWindowLayoutTests: XCTestCase {
    func testNormalizeRepairsCollapsedWindowAndPreservesAUserSize() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 100, height: 100),
            styleMask: [.titled, .closable], backing: .buffered, defer: false
        )
        window.isReleasedWhenClosed = false
        SettingsWindowLayout.normalize(window)
        XCTAssertEqual(window.contentLayoutRect.width, SettingsWindowLayout.defaultContentSize.width)
        XCTAssertGreaterThanOrEqual(window.contentLayoutRect.height, SettingsWindowLayout.minimumContentSize.height)
        XCTAssertEqual(window.contentMinSize, SettingsWindowLayout.minimumContentSize)
        XCTAssertTrue(window.styleMask.contains(.resizable))
        XCTAssertTrue(window.styleMask.contains(.miniaturizable))
        XCTAssertEqual(window.toolbarStyle, .unified)
        window.setContentSize(NSSize(width: 880, height: 760))
        let selectedFrame = window.frame
        window.title = "Security"
        SettingsWindowLayout.normalize(window)
        XCTAssertEqual(window.frame, selectedFrame)
        XCTAssertEqual(window.title, "Security")
    }

    func testNavigationRestoresPaneAndProtectsDraftsAcrossHistory() {
        let suite = "SettingsWindowLayoutTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set("unknown", forKey: SettingsPane.defaultsKey)
        let navigation = SettingsNavigation(defaults: defaults)
        XCTAssertEqual(navigation.destination, .pane(.general))
        navigation.navigate(to: .pane(.organization))
        navigation.navigate(to: .organization(.members))
        navigation.isSaving = true
        navigation.hasUnsavedChanges = true
        navigation.goBack()
        XCTAssertEqual(navigation.destination, .organization(.members))
        XCTAssertNil(navigation.pendingDestination)
        navigation.isSaving = false
        navigation.hasUnsavedChanges = true
        navigation.goBack()
        XCTAssertEqual(navigation.destination, .organization(.members))
        XCTAssertEqual(navigation.pendingDestination, .pane(.organization))
        navigation.pendingDestination = nil
        XCTAssertTrue(navigation.hasUnsavedChanges)
        navigation.goBack()
        navigation.discardAndNavigate()
        XCTAssertEqual(navigation.destination, .pane(.organization))
        XCTAssertFalse(navigation.hasUnsavedChanges)
        navigation.goForward()
        XCTAssertEqual(navigation.destination, .organization(.members))
        XCTAssertEqual(SettingsPane.restored(from: defaults), .organization)
        navigation.isSaving = true
        navigation.navigate(to: .pane(.general))
        XCTAssertEqual(navigation.destination, .pane(.general), "A background membership update must not lock unrelated Settings panes")
        navigation.isSaving = false
        navigation.resetForAuthorityChange()
        XCTAssertEqual(navigation.destination, .pane(.general))
        XCTAssertFalse(navigation.canGoBack)
        XCTAssertFalse(navigation.canGoForward)
    }

    func testSettingsSearchIncludesProjectManagementOnlyForOrganizationAdministrators() {
        let destinations = SettingsDestination.search("", canAdminister: true)
        XCTAssertTrue(destinations.contains(.organization(.projects)))
        XCTAssertTrue(destinations.contains(.organization(.members)))
        XCTAssertTrue(destinations.contains(.organization(.access)))
        XCTAssertEqual(SettingsDestination.search("project", canAdminister: true), [.organization(.projects)])
        XCTAssertTrue(SettingsDestination.search("project", canAdminister: false).isEmpty)
        XCTAssertFalse(SettingsDestination.search("", canAdminister: false).contains(.organization(.projects)))
    }

    func testSettingsSearchFindsChildrenAndRespectsOrganizationPermission() {
        XCTAssertEqual(SettingsDestination.search("email domains", canAdminister: true), [.organization(.access)])
        XCTAssertTrue(SettingsDestination.search("credentials", canAdminister: true).isEmpty)
        XCTAssertTrue(SettingsDestination.search("credentials", canAdminister: false).isEmpty)
        XCTAssertEqual(SettingsDestination.search("updates", canAdminister: false), [.pane(.general)])
        XCTAssertEqual(SettingsDestination.search("plugin", canAdminister: false), [.pane(.agent)])
        XCTAssertEqual(SettingsDestination.search("SSO", canAdminister: true), [.organization(.access)])
    }

    func testClosingSettingsCanKeepEditingOrDiscardTheDraft() {
        let suite = "SettingsWindowLayoutTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let navigation = SettingsNavigation(defaults: defaults)
        let controller = SettingsWindowController(
            store: WorkspaceStore(), softwareUpdateController: SoftwareUpdateController(startingUpdater: false),
            onShowLogs: {}, navigation: navigation
        )
        let window = NSWindow()
        window.isReleasedWhenClosed = false
        controller.window = window
        navigation.hasUnsavedChanges = true
        controller.confirmDiscard = { false }
        XCTAssertFalse(controller.windowShouldClose(window))
        XCTAssertTrue(navigation.hasUnsavedChanges)
        controller.confirmDiscard = { true }
        XCTAssertTrue(controller.windowShouldClose(window))
        controller.windowWillClose(Notification(name: NSWindow.willCloseNotification, object: window))
        XCTAssertNil(controller.window)
        XCTAssertFalse(navigation.hasUnsavedChanges)
    }
}
