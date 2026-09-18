import Combine
import Foundation

@MainActor
final class BundlesModel: ObservableObject {
    private let bundles: BundleStore

    private var loadObservation: AnyCancellable?

    init(bundles: BundleStore) {
        self.bundles = bundles
        loadObservation = bundles.didLoad.sink { [weak self] in
            guard let self else { return }
            if let selectedBundleId, self.bundles.bundles.contains(where: { $0.id == selectedBundleId }) { return }
            selectedBundleId = self.bundles.bundles.first?.id
        }
    }

    @Published var selectedBundleId: String?

    var selectedBundle: PersonalBundle? {
        bundles.bundles.first { $0.id == self.selectedBundleId } ?? bundles.bundles.first
    }

    func createBundle() async {
        if let id = await bundles.createBundleRecord() {
            selectedBundleId = id
        }
    }

    func deleteBundle(_ bundle: PersonalBundle) async {
        if await bundles.deleteBundleRecord(bundle) {
            selectedBundleId = bundles.bundles.first?.id
        }
    }
}
