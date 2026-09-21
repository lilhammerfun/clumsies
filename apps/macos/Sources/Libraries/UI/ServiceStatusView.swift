import SwiftUI

private struct PageFeedback {
    let message: String
    var isStatus = false
    var retryTitle = String(localized: "Retry")
    var retry: (() -> Void)?
    let dismiss: () -> Void
}

private struct PageFeedbackPreferenceKey: PreferenceKey {
    static var defaultValue: [PageFeedback] { [] }
    static func reduce(value: inout [PageFeedback], nextValue: () -> [PageFeedback]) {
        value.append(contentsOf: nextValue())
    }
}

private struct PageFeedbackSource: ViewModifier {
    let message: String?
    let isStatus: Bool
    let retryTitle: String
    let retry: (() -> Void)?
    let onDismiss: (() -> Void)?
    @State private var dismissed = false

    func body(content: Content) -> some View {
        content.background {
            Color.clear.preference(key: PageFeedbackPreferenceKey.self, value: message.map {
                dismissed ? [] : [PageFeedback(message: $0, isStatus: isStatus,
                    retryTitle: retryTitle, retry: retry, dismiss: { dismissed = true; onDismiss?() })]
            } ?? [])
        }
        .onChange(of: message) { _, _ in dismissed = false }
    }
}

extension View {
    /// Report feedback without inserting a row, resizing a form, or moving a sidebar.
    func pageFeedback(_ message: String?, isStatus: Bool = false,
                      retryTitle: String = String(localized: "Retry"), dismiss: (() -> Void)? = nil,
                      retry: (() -> Void)? = nil) -> some View {
        modifier(PageFeedbackSource(message: message, isStatus: isStatus, retryTitle: retryTitle, retry: retry, onDismiss: dismiss))
    }

    /// A window owns one corner. Form submission errors stay inside the form.
    func feedbackHost(error: String? = nil, dismiss: @escaping () -> Void = {},
                      showsService: Bool = true) -> some View {
        overlayPreferenceValue(PageFeedbackPreferenceKey.self, alignment: .bottomTrailing) { feedback in
            WindowFeedbackView(feedback: feedback, error: error, dismiss: dismiss, showsService: showsService)
                .padding(16)
        }
        .transformPreference(PageFeedbackPreferenceKey.self) { $0.removeAll() }
    }
}

private struct WindowFeedbackView: View {
    @ObservedObject private var status = ClientServiceStatus.shared
    let feedback: [PageFeedback]
    let error: String?
    let dismiss: () -> Void
    let showsService: Bool

    var body: some View {
        if let notice = feedback.last(where: { !$0.isStatus }) {
            label(notice)
        } else if let error {
            label(PageFeedback(message: error, dismiss: dismiss))
        } else if showsService, let failure = status.failure {
            label(PageFeedback(message: failure.message, isStatus: true, dismiss: status.dismiss),
                 symbol: failure == .authentication ? "person.crop.circle.badge.exclamationmark" : "network.slash")
                .accessibilityIdentifier("service-connection-status")
        } else if let notice = feedback.last {
            label(notice)
        }
    }

    private func label(_ notice: PageFeedback, symbol: String = "exclamationmark.circle") -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: symbol).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 8) {
                Text(notice.message)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                if let retry = notice.retry {
                    Button(notice.retryTitle, action: retry)
                        .accessibilityIdentifier("page-feedback-retry")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button {
                // A shared failure can be reported by several visible sections. Dismiss it once.
                for duplicate in feedback where duplicate.message == notice.message { duplicate.dismiss() }
                if error == notice.message { dismiss() }
                if status.failure?.message == notice.message { status.dismiss() }
            } label: { Image(systemName: "xmark") }
                .buttonStyle(.plain)
                .accessibilityLabel("Dismiss")
                .accessibilityIdentifier("page-feedback-dismiss")
        }
        .font(.callout)
        .foregroundStyle(notice.isStatus ? AnyShapeStyle(.secondary) : AnyShapeStyle(.red))
        .frame(maxWidth: 320, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("page-feedback")
    }
}

/// Submission feedback belongs to the form, without a floating container or another alert.
struct FormErrorMessage: View {
    let message: String?
    var retry: (() -> Void)?

    var body: some View {
        if let message {
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .top, spacing: 6) {
                    Image(systemName: "exclamationmark.circle").accessibilityHidden(true)
                    Text(message)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
                .foregroundStyle(.red)
                if let retry { Button("Retry", action: retry) }
            }
            .font(.callout)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("form-error")
        }
    }
}
