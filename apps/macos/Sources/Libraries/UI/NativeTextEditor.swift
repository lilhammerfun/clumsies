import AppKit
import SwiftUI

enum DocumentContentMetrics {
    static let maximumWidth: CGFloat = 760
    static let minimumHorizontalInset: CGFloat = 36
}

@MainActor
final class CenteredTextView: NSTextView {
    static func horizontalInset(for width: CGFloat) -> CGFloat {
        max(
            DocumentContentMetrics.minimumHorizontalInset,
            (width - DocumentContentMetrics.maximumWidth) / 2
        )
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        textContainerInset = NSSize(
            width: Self.horizontalInset(for: newSize.width),
            height: 18
        )
    }
}

struct NativeTextEditor: NSViewRepresentable {
    @Binding var text: String
    var font: NSFont = .monospacedSystemFont(ofSize: 13, weight: .regular)
    @Environment(\.isEnabled) private var isEnabled

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text)
    }

    func makeNSView(context: Context) -> NSScrollView {
        let textView = CenteredTextView()
        textView.delegate = context.coordinator
        textView.isRichText = false
        textView.allowsUndo = true
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.usesFindBar = true
        textView.font = font
        textView.textContainerInset = NSSize(
            width: DocumentContentMetrics.minimumHorizontalInset,
            height: 18
        )
        textView.drawsBackground = true
        textView.backgroundColor = .textBackgroundColor
        textView.string = text
        textView.isEditable = isEnabled
        textView.isSelectable = true
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.textContainer?.widthTracksTextView = true
        textView.textContainer?.containerSize = NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude)

        let scrollView = NSScrollView()
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.borderType = .noBorder
        scrollView.documentView = textView
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        guard let textView = scrollView.documentView as? NSTextView else { return }
        textView.isEditable = isEnabled
        guard
              textView.string != text,
              !context.coordinator.isApplyingTextChange else { return }
        let selection = textView.selectedRange()
        context.coordinator.isApplyingTextChange = true
        textView.string = text
        textView.setSelectedRange(NSIntersectionRange(selection, NSRange(location: 0, length: text.utf16.count)))
        context.coordinator.isApplyingTextChange = false
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        @Binding var text: String
        var isApplyingTextChange = false

        init(text: Binding<String>) {
            _text = text
        }

        func textDidChange(_ notification: Notification) {
            guard !isApplyingTextChange, let textView = notification.object as? NSTextView else { return }
            text = textView.string
        }
    }
}
