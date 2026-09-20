import AppKit
import SwiftUI

@MainActor
enum ToolbarHelp {
    private static let fallbackLabels = NSMapTable<NSToolbarItem, NSString>.weakToStrongObjects()

    /// SwiftUI also creates native Back and Sidebar items outside our view modifiers.
    static func fillMissingTooltips(in items: [NSToolbarItem]) {
        for item in items {
            let previous = fallbackLabels.object(forKey: item) as String?
            if item.toolTip == nil || item.toolTip == previous {
                let text = item.label.isEmpty ? nil : item.label
                if item.toolTip != text { item.toolTip = text }
                if let text {
                    fallbackLabels.setObject(text as NSString, forKey: item)
                } else {
                    fallbackLabels.removeObject(forKey: item)
                }
            }
            if let group = item as? NSToolbarItemGroup {
                fillMissingTooltips(in: group.subitems)
            }
        }
    }
}

extension View {
    /// Keep help available when SwiftUI hosts toolbar content in the native title bar.
    func toolbarHelp(_ text: String) -> some View {
        help(text).background(ToolbarHelpView(text: text))
    }
}

private struct ToolbarHelpView: NSViewRepresentable {
    let text: String

    func makeNSView(context: Context) -> HelpView { HelpView() }

    func updateNSView(_ view: HelpView, context: Context) {
        view.toolTip = text
    }

    final class HelpView: NSView {
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
        override func isAccessibilityElement() -> Bool { false }
    }
}
