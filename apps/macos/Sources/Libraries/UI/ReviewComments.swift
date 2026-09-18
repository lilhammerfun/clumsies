import SwiftUI

struct ReviewCommentRow: View {
    let comment: ReviewComment
    let onReply: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            AvatarView(account: comment.author)

            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(comment.author.displayName ?? comment.author.email)
                        .font(.caption.weight(.semibold))
                    Text(
                        TimestampFormatting.relativeText(comment.createdAt, relativeTo: .now)
                            ?? comment.createdAt
                    )
                    .font(.caption)
                    .foregroundStyle(.tertiary)

                    Spacer(minLength: 8)

                    Button(action: onReply) {
                        Image(systemName: "arrowshape.turn.up.left")
                    }
                    .buttonStyle(.borderless)
                    .help("Reply")
                    .accessibilityLabel("Reply")
                }
                Text(comment.body)
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct ReviewCommentComposer: View {
    @Binding var text: String
    let isSubmitting: Bool
    let onCancel: () -> Void
    let onSubmit: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            TextField(
                "Write a comment…",
                text: $text,
                axis: .vertical
            )
            .lineLimit(2...6)
            .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                    .disabled(isSubmitting)
                Button {
                    onSubmit()
                } label: {
                    if isSubmitting {
                        ProgressView().controlSize(.small)
                    } else {
                        Text("Comment")
                    }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(
                    text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || isSubmitting
                )
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
