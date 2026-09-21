import SwiftUI

/// Native sheet actions. For a single Done or Close action, the sheet handles Escape dismissal.
struct SheetActionBar: View {
    let confirmationTitle: Text
    var cancellationTitle: LocalizedStringKey = "Cancel"
    var progressTitle: LocalizedStringKey = "Working…"
    var isWorking = false
    var canConfirm = true
    var allowsCancellationWhileWorking = false
    var confirmationIdentifier = "sheet-confirm"
    var cancel: (() -> Void)?
    let confirm: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Divider()
            HStack(spacing: 8) {
                if isWorking {
                    ProgressView()
                        .controlSize(.small)
                        .accessibilityHidden(true)
                    Text(progressTitle)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 16)
                if let cancel {
                    Button(role: .cancel, action: cancel) {
                        Text(cancellationTitle).frame(minWidth: 64)
                    }
                    .buttonStyle(.bordered)
                    .keyboardShortcut(.cancelAction)
                    .disabled(isWorking && !allowsCancellationWhileWorking)
                    .accessibilityIdentifier("sheet-cancel")
                }
                Button(action: confirm) {
                    confirmationTitle.frame(minWidth: 64)
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(isWorking || !canConfirm)
                .accessibilityIdentifier(confirmationIdentifier)
            }
            .controlSize(.regular)
            .frame(minHeight: 24)
            .padding(.horizontal, 24)
            .padding(.vertical, 16)
        }
        .background(.bar)
    }
}
