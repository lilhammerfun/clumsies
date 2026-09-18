import SwiftUI

/// Keep the content area visible while the first usable result is loading.
struct ContentLoadingView: View {
    enum Layout { case list, document }

    let title: String
    var layout: Layout = .list

    var body: some View {
        VStack(alignment: .leading, spacing: 24) {
            ProgressView(title)
                .controlSize(.small)
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: layout == .document ? 14 : 24) {
                ForEach(0..<5) { row in
                    VStack(alignment: .leading, spacing: 8) {
                        RoundedRectangle(cornerRadius: 4)
                            .fill(.quaternary)
                            .frame(height: row == 0 && layout == .document ? 24 : 12)
                        RoundedRectangle(cornerRadius: 4)
                            .fill(.quaternary)
                            .frame(maxWidth: row.isMultiple(of: 2) ? 180 : 260)
                            .frame(height: 10)
                    }
                }
            }
            .accessibilityHidden(true)
            Spacer(minLength: 0)
        }
        .padding(24)
        .frame(maxWidth: layout == .document ? 760 : .infinity, alignment: .leading)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
