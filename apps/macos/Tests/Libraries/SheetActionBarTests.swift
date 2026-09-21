import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class SheetActionBarTests: XCTestCase {
    func testKeyboardShortcutsRespectBusyAndValidationStates() throws {
        // A read-only open operation can still be closed; committed writes cannot.
        for (hasCancel, working, valid, allowsCancel) in [
            (true, false, true, false), (true, false, false, false),
            (true, true, true, false), (true, true, true, true),
            (false, false, true, false)
        ] {
            var confirmations = 0
            var cancellations = 0
            let controller = NSHostingController(rootView: SheetActionBar(
                confirmationTitle: Text("Done"), isWorking: working, canConfirm: valid,
                allowsCancellationWhileWorking: allowsCancel,
                cancel: hasCancel ? { cancellations += 1 } : nil,
                confirm: { confirmations += 1 }
            ).frame(width: 440))
            let window = NSWindow(contentViewController: controller)
            window.isReleasedWhenClosed = false
            window.makeKeyAndOrderFront(nil)
            defer { window.close() }
            window.makeFirstResponder(controller.view)
            for _ in 0..<3 {
                controller.view.layoutSubtreeIfNeeded()
                RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            }
            for (characters, keyCode) in [("\r", UInt16(36)), ("\u{1b}", UInt16(53))] {
                let event = try XCTUnwrap(NSEvent.keyEvent(
                    with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                    windowNumber: window.windowNumber, context: nil, characters: characters,
                    charactersIgnoringModifiers: characters, isARepeat: false, keyCode: keyCode
                ))
                if !window.performKeyEquivalent(with: event) { window.sendEvent(event) }
            }
            let context = "cancel=\(hasCancel), working=\(working), valid=\(valid), allowsCancel=\(allowsCancel)"
            XCTAssertEqual(confirmations, !working && valid ? 1 : 0, context)
            XCTAssertEqual(cancellations, hasCancel && (!working || allowsCancel) ? 1 : 0, context)
        }
    }

    func testSingleActionSheetUsesNativeEscapeDismissal() throws {
        var presented = true
        let controller = NSHostingController(rootView: Text("Parent").frame(width: 640, height: 480)
            .sheet(isPresented: Binding(get: { presented }, set: { presented = $0 })) {
                SheetActionBar(confirmationTitle: Text("Done"), confirm: { presented = false })
                    .frame(width: 440)
            })
        let window = NSWindow(contentViewController: controller)
        window.isReleasedWhenClosed = false
        window.makeKeyAndOrderFront(nil)
        defer { window.close() }
        for _ in 0..<40 where window.attachedSheet == nil {
            RunLoop.current.run(until: Date().addingTimeInterval(0.025))
        }
        let sheet = try XCTUnwrap(window.attachedSheet)
        let escape = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: sheet.windowNumber, context: nil, characters: "\u{1b}",
            charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53
        ))
        if !sheet.performKeyEquivalent(with: escape) { sheet.sendEvent(escape) }
        for _ in 0..<40 where presented {
            RunLoop.current.run(until: Date().addingTimeInterval(0.025))
        }
        XCTAssertFalse(presented, "Read-only sheets must retain native Escape dismissal.")
    }
}
