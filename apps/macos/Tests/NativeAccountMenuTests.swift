import AppKit
import XCTest
@testable import Clumsies

@MainActor
final class NativeAccountMenuTests: XCTestCase {
    func testAccountMenuIdentifiesTheAccountAndOffersOnlySettingsAndSignOut() {
        var didOpenSettings = false
        var didSignOut = false
        let coordinator = NativeAccountMenu.Coordinator(configuration: .init(
            account: .init(userId: "user-1", email: "dylan@example.com", displayName: "Dylan",
                avatarUrl: nil, role: "member"),
            onOpenSettings: { didOpenSettings = true },
            onSignOut: { didSignOut = true }
        ))

        let menu = coordinator.makeMenu()
        XCTAssertEqual(menu.items.map(\.title), ["dylan@example.com", "Settings…", "", "Sign Out"])
        XCTAssertFalse(menu.items[0].isEnabled)
        XCTAssertTrue(menu.items[2].isSeparatorItem)
        XCTAssertTrue(menu.items.allSatisfy { $0.submenu == nil })
        menu.performActionForItem(at: 1)
        XCTAssertTrue(didOpenSettings)
        XCTAssertFalse(didSignOut)
        menu.performActionForItem(at: 3)
        XCTAssertTrue(didSignOut)
    }

    func testMissingAccountDoesNotShowAnInventedIdentity() {
        let coordinator = NativeAccountMenu.Coordinator(configuration: .init(
            account: nil, onOpenSettings: {}, onSignOut: {}
        ))
        XCTAssertEqual(coordinator.makeMenu().items.map(\.title), ["Settings…", "", "Sign Out"])
    }
}
