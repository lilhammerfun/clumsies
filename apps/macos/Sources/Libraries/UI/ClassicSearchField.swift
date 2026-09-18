import AppKit
import SwiftUI

/// A unified, always-visible search field for toolbar placement.
///
/// Wraps `NSSearchField` so every search surface in the app shares the
/// classic macOS search box: rounded bezel, system magnifying-glass icon,
/// clear button, focus ring, and automatic light/dark appearance. The field
/// is rendered directly in the toolbar — never collapsed behind an icon —
/// and stays on the trailing (right) edge of every workspace.
struct ClassicSearchField: NSViewRepresentable {
    @Binding var text: String
    var prompt: String
    var width: CGFloat = 190
    var accessibilityIdentifier = "toolbar-search"
    var accessibilityHelp: String?
    /// Increment to request first responder (e.g. the Cmd+K shortcut).
    var focusToken = 0

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text, focusToken: focusToken)
    }

    func makeNSView(context: Context) -> NSSearchField {
        let field = NSSearchField()
        field.placeholderString = prompt
        field.bezelStyle = .roundedBezel
        field.sendsSearchStringImmediately = true
        field.sendsWholeSearchString = false
        field.delegate = context.coordinator
        field.translatesAutoresizingMaskIntoConstraints = false
        field.widthAnchor.constraint(equalToConstant: width).isActive = true
        field.setAccessibilityIdentifier(accessibilityIdentifier)
        if let accessibilityHelp {
            field.setAccessibilityHelp(accessibilityHelp)
        }
        return field
    }

    func updateNSView(_ field: NSSearchField, context: Context) {
        if field.stringValue != text,
           field.currentEditor() == nil || text.isEmpty {
            field.stringValue = text
        }
        if field.placeholderString != prompt {
            field.placeholderString = prompt
        }
        if field.accessibilityIdentifier() != accessibilityIdentifier {
            field.setAccessibilityIdentifier(accessibilityIdentifier)
        }
        if focusToken != context.coordinator.lastFocusToken {
            context.coordinator.lastFocusToken = focusToken
            DispatchQueue.main.async {
                field.window?.makeFirstResponder(field)
            }
        }
    }

    final class Coordinator: NSObject, NSSearchFieldDelegate {
        @Binding var text: String
        var lastFocusToken: Int

        init(text: Binding<String>, focusToken: Int) {
            _text = text
            // Seed with the initial token so the first render never steals focus;
            // only explicit increments (e.g. Cmd+K) request first responder.
            lastFocusToken = focusToken
        }

        func controlTextDidChange(_ obj: Notification) {
            guard let field = obj.object as? NSSearchField,
                  text != field.stringValue
            else { return }
            text = field.stringValue
        }
    }
}
