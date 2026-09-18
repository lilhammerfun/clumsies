import SwiftUI

struct ReviewSymbolImage: View {
    let systemName: String

    var body: some View {
        Group {
            switch systemName {
            case "checkmark.bubble":
                Image("ReviewPullRequest", bundle: .main)
                    .renderingMode(.template)
            case "arrow.triangle.merge":
                Image("ReviewMerged", bundle: .main)
                    .renderingMode(.template)
            default:
                Image(systemName: systemName)
            }
        }
        .frame(width: 16, height: 16)
        .accessibilityHidden(true)
    }
}
