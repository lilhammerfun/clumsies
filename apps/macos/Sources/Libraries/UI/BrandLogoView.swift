import SwiftUI

struct BrandLogoView: View {
    var size: CGFloat = 64
    var isBreathing: Bool = false

    @State private var isSubtlyScaled = false

    private let brandPink = Color(red: 0.90, green: 0.34, blue: 0.63)

    var body: some View {
        Image("BrandMark", bundle: .main)
            .resizable()
            .scaledToFit()
            .frame(width: size, height: size)
            .shadow(color: .black.opacity(0.08), radius: 6, x: 0, y: 3)
            .shadow(color: brandPink.opacity(0.12), radius: 12, x: 0, y: 4)
            .scaleEffect(isBreathing && isSubtlyScaled ? 1.025 : 0.985)
            .onAppear {
                guard isBreathing else { return }
                withAnimation(
                    .easeInOut(duration: 2.0)
                    .repeatForever(autoreverses: true)
                ) {
                    isSubtlyScaled = true
                }
            }
    }
}
