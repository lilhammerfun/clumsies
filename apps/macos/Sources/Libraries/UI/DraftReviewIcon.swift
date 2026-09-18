import AppKit
import SwiftUI

struct DraftReviewIcon: View {
    var submitted = true

    private static let openImage = load("git-pull-request-16")
    private static let draftImage = load("git-pull-request-draft-16")

    private static func load(_ name: String) -> NSImage? {
        Bundle.main.url(forResource: name, withExtension: "svg", subdirectory: "Octicons")
            .flatMap { NSImage(contentsOf: $0) }
    }

    var body: some View {
        Group {
            if let image = submitted ? Self.openImage : Self.draftImage {
                Image(nsImage: image)
                    .renderingMode(.template)
                    .resizable()
                    .scaledToFit()
            } else {
                Image(systemName: "checkmark.bubble")
                    .resizable()
                    .scaledToFit()
            }
        }
        .frame(width: 14, height: 14)
        .foregroundStyle(submitted ? Color(nsColor: .systemGreen) : .secondary)
    }
}
