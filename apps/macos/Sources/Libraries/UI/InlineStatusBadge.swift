import SwiftUI

/// A compact status label that follows a title instead of occupying a List's trailing badge slot.
struct InlineStatusBadge: View {
    let text: String
    var color: Color? = nil

    var body: some View {
        Text(text)
            .font(.system(size: 10, weight: .semibold))
            .foregroundStyle(color == nil ? Color.secondary : Color.white)
            .padding(.horizontal, 5)
            .padding(.vertical, 1)
            .background(color ?? .clear, in: Capsule())
            .fixedSize()
    }
}
