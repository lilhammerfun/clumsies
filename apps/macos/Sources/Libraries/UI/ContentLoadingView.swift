import SwiftUI

/// One native indicator in the content region that is awaiting its first result.
struct ContentLoadingView: View {
    let title: String

    var body: some View {
        ProgressView()
            .controlSize(.small)
            .accessibilityLabel(title)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
