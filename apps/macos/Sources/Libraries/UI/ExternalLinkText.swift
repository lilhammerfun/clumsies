import SwiftUI

/// A reusable classic link: tinted, underlined text that opens the given URL
/// in the default browser. Shows the pointing-hand cursor on hover and the
/// target URL as a tooltip. Clicking is only enabled for http(s) URLs with a
/// non-empty host.
struct ExternalLinkText: View {
    let url: URL
    let title: String
    let systemImage: String?

    init(url: URL, title: String, systemImage: String? = nil) {
        self.url = url
        self.title = title
        self.systemImage = systemImage
    }

    var body: some View {
        Button {
            NSWorkspace.shared.open(url)
        } label: {
            HStack(spacing: 4) {
                if let systemImage {
                    Image(systemName: systemImage)
                }
                Text(title)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .underline()
                    .foregroundStyle(Color(nsColor: .linkColor))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .focusEffectDisabled()
        .pointingHandCursor()
        .help(url.absoluteString)
    }
}
