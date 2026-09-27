import AppKit
import SwiftUI

@MainActor
final class StartupWindowController: NSWindowController {
    static let contentSize = NSSize(width: 540, height: 360)

    init() {
        super.init(window: nil)
    }

    required init?(coder: NSCoder) { nil }

    func show<Content: View>(_ content: Content) {
        let size = Self.contentSize
        let window = window ?? NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.titled, .closable, .fullSizeContentView],
            backing: .buffered, defer: false
        )
        window.title = ClumsiesIdentifiers.appDisplayName
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isMovableByWindowBackground = true
        window.backgroundColor = .textBackgroundColor
        window.isReleasedWhenClosed = false
        window.contentView = NSHostingView(rootView: content
            .frame(width: size.width, height: size.height)
            .background(Color(nsColor: .textBackgroundColor)))
        window.contentMinSize = size
        window.contentMaxSize = size
        window.setContentSize(size)
        if self.window == nil {
            window.center()
            self.window = window
        }
        window.makeKeyAndOrderFront(nil)
    }
}
