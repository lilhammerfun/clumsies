import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class NativeTextEditorIMETests: XCTestCase {
    func testUncommittedChineseCompositionSurvivesViewUpdate() async throws {
        try await exerciseComposition(externalUpdate: false)
    }

    func testCompositionSurvivesConcurrentModelUpdate() async throws {
        try await exerciseComposition(externalUpdate: true)
    }

    private func exerciseComposition(externalUpdate: Bool) async throws {
        var body = "before \nafter"
        let binding = Binding<String>(get: { body }, set: { body = $0 })
        let host = NSHostingView(rootView: NativeTextEditor(text: binding))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 800, height: 500),
            styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.makeKeyAndOrderFront(nil)
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(100))
        func editor(_ view: NSView) -> NSTextView? {
            if let text = view as? NSTextView { return text }
            return view.subviews.lazy.compactMap { editor($0) }.first
        }
        let text = try XCTUnwrap(editor(host))
        window.makeFirstResponder(text)
        text.setSelectedRange(NSRange(location: 7, length: 0))
        text.setMarkedText("zhong", selectedRange: NSRange(location: 5, length: 0),
                           replacementRange: NSRange(location: NSNotFound, length: 0))
        let composing = text.string
        let selection = text.selectedRange()
        let marked = text.markedRange()
        XCTAssertTrue(text.hasMarkedText())
        if externalUpdate { body += "\nrefresh" }
        host.rootView = NativeTextEditor(text: binding, font: .systemFont(ofSize: 14))
        host.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(text.string, composing)
        XCTAssertEqual(text.selectedRange(), selection)
        XCTAssertEqual(text.markedRange(), marked)
        text.insertText("中", replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertEqual(body, "before 中\nafter")
        XCTAssertFalse(text.hasMarkedText())
    }
}
