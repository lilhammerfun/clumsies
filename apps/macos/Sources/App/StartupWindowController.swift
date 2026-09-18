import AppKit
import SwiftUI

@MainActor
final class StartupWindowController: NSWindowController {
    static let contentSize = NSSize(width: 540, height: 690)

    init() {
        super.init(window: nil)
    }

    required init?(coder: NSCoder) { nil }

    func show<Content: View>(_ content: Content, height: CGFloat = contentSize.height) {
        let size = NSSize(width: Self.contentSize.width, height: height)
        let window = window ?? NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.titled, .closable, .fullSizeContentView],
            backing: .buffered, defer: false
        )
        let previousFrame = window.frame
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
        } else {
            window.setFrameOrigin(NSPoint(
                x: previousFrame.midX - window.frame.width / 2,
                y: previousFrame.midY - window.frame.height / 2
            ))
        }
        window.makeKeyAndOrderFront(nil)
    }
}
