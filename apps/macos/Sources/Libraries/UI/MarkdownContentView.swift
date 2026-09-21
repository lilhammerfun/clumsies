import Foundation
import MarkdownUI
import SwiftUI

struct MarkdownContentView: View {
    let source: String
    @State private var showsFrontmatter = true

    var body: some View {
        let document = MarkdownPreviewDocument(source: source)
        VStack(alignment: .leading, spacing: 20) {
            if let frontmatter = document.frontmatter, !frontmatter.isEmpty {
                DisclosureGroup("Frontmatter", isExpanded: $showsFrontmatter) {
                    Text(verbatim: frontmatter)
                        .font(.system(.callout, design: .monospaced))
                        .foregroundStyle(.primary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            if !document.body.isEmpty {
                Markdown(document.body)
                    .markdownTheme(.gitHub)
            }
        }
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Separates a closed frontmatter block at the start without interpreting its YAML.
struct MarkdownPreviewDocument {
    let frontmatter: String?
    let body: String

    private static let frontmatterPattern = try! NSRegularExpression(
        pattern: #"\A\uFEFF?---[ \t]*\r?\n(.*?)^(?:---|\.\.\.)[ \t]*(?:\r?\n|\z)"#,
        options: [.anchorsMatchLines, .dotMatchesLineSeparators]
    )

    init(source: String) {
        guard let match = Self.frontmatterPattern.firstMatch(
            in: source, range: NSRange(source.startIndex..., in: source)
        ), let blockRange = Range(match.range, in: source),
           let contentRange = Range(match.range(at: 1), in: source) else {
            frontmatter = nil
            body = source
            return
        }
        frontmatter = String(source[contentRange]).trimmingCharacters(in: .newlines)
        body = String(source[blockRange.upperBound...])
    }
}
