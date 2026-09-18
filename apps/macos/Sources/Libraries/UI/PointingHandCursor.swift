import SwiftUI

/// Changes the mouse cursor to the pointing-hand cursor while the wrapped view
/// is hovered. Push/pop pairs are balanced on disappear so a cursor never
/// stays stuck after the view leaves the hierarchy mid-hover.
struct PointingHandCursorModifier: ViewModifier {
    @State private var isHovering = false

    func body(content: Content) -> some View {
        content
            .onHover { hovering in
                updateCursor(hovering)
            }
            .onDisappear {
                updateCursor(false)
            }
    }

    private func updateCursor(_ hovering: Bool) {
        guard isHovering != hovering else { return }
        isHovering = hovering
        if hovering {
            NSCursor.pointingHand.push()
        } else {
            NSCursor.pop()
        }
    }
}

extension View {
    /// Shows the pointing-hand cursor while the view is hovered.
    func pointingHandCursor() -> some View {
        modifier(PointingHandCursorModifier())
    }
}
