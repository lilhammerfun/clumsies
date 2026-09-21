import SwiftUI

struct MarkdownPreview: View {
    let source: String

    var body: some View {
        ScrollView {
            MarkdownContentView(source: source)
                .frame(maxWidth: DocumentContentMetrics.maximumWidth, alignment: .leading)
                .padding(.horizontal, DocumentContentMetrics.minimumHorizontalInset)
                .padding(.vertical, 28)
                .frame(maxWidth: .infinity, alignment: .top)
        }
        .background(Color(nsColor: .textBackgroundColor))
    }
}
