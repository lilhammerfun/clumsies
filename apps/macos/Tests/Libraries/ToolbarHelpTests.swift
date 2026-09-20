import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class ToolbarHelpTests: XCTestCase {
    func testNativeNavigationItemsHaveHelpWithoutOverwritingExplicitHelp() {
        let back = NSToolbarItem(itemIdentifier: .init("back"))
        back.label = "Back"
        let sidebar = NSToolbarItem(itemIdentifier: .toggleSidebar)
        sidebar.label = "Toggle Sidebar"
        let save = NSToolbarItem(itemIdentifier: .init("save"))
        save.label = "Save"
        save.toolTip = "Resolve conflicts before saving"
        let group = NSToolbarItemGroup(itemIdentifier: .init("navigation"))
        group.subitems = [back, sidebar, save]
        ToolbarHelp.fillMissingTooltips(in: [group])
        ToolbarHelp.fillMissingTooltips(in: [group])
        XCTAssertEqual(back.toolTip, "Back")
        XCTAssertEqual(sidebar.toolTip, "Toggle Sidebar")
        XCTAssertEqual(save.toolTip, "Resolve conflicts before saving")
        sidebar.label = "Show Sidebar"
        ToolbarHelp.fillMissingTooltips(in: [group])
        XCTAssertEqual(sidebar.toolTip, "Show Sidebar")
        sidebar.toolTip = "Custom Sidebar Help"
        sidebar.label = "Hide Sidebar"
        ToolbarHelp.fillMissingTooltips(in: [group])
        XCTAssertEqual(sidebar.toolTip, "Custom Sidebar Help")
    }

    func testNativeToolbarHelpSurvivesDisabledControlsAndUpdates() async throws {
        let state = HelpState()
        let host = NSHostingView(rootView: HelpToolbar(state: state))
        host.sizingOptions = []
        if #available(macOS 26, *) { host.sceneBridgingOptions = .all }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 600, height: 300),
            styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }

        func helpViews(_ view: NSView) -> [NSView] {
            (view.toolTip == nil ? [] : [view]) + view.subviews.flatMap(helpViews)
        }
        for disabled in [true, false] {
            state.disabled = disabled
            let expected = disabled ? "Save — resolve conflicts first" : "Save Review Updates"
            var tips: [NSView] = []
            for _ in 0..<40 {
                host.layoutSubtreeIfNeeded()
                tips = (window.toolbar?.items ?? []).compactMap(\.view).flatMap(helpViews)
                if tips.contains(where: { $0.toolTip == expected }) { break }
                try await Task.sleep(for: .milliseconds(25))
            }
            let saveHelp = try XCTUnwrap(tips.first { $0.toolTip == expected })
            XCTAssertGreaterThan(saveHelp.bounds.width, 0)
            XCTAssertGreaterThan(saveHelp.bounds.height, 0)
            XCTAssertNil(saveHelp.hitTest(NSPoint(x: saveHelp.bounds.midX, y: saveHelp.bounds.midY)),
                         "The tooltip must not intercept the toolbar button's clicks")
            XCTAssertTrue(tips.contains { $0.toolTip == "File Actions" })
            XCTAssertFalse(tips.contains {
                $0.toolTip == (disabled ? "Save Review Updates" : "Save — resolve conflicts first")
            })
        }
    }
}

@MainActor
private final class HelpState: ObservableObject {
    @Published var disabled = true
}

private struct HelpToolbar: View {
    @ObservedObject var state: HelpState

    var body: some View {
        Text("Review").toolbar {
            ToolbarItemGroup {
                Button {} label: { Image(systemName: "square.and.arrow.down") }
                    .disabled(state.disabled)
                    .toolbarHelp(state.disabled ? "Save — resolve conflicts first" : "Save Review Updates")
                    .accessibilityLabel("Save Review Updates")
                Menu { Button("Export") {} } label: { Image(systemName: "ellipsis") }
                    .toolbarHelp("File Actions")
            }
        }
    }
}
