import Combine
import Sparkle

@MainActor
final class SoftwareUpdateController: ObservableObject {
    private let updater: SPUUpdater
    private var observation: AnyCancellable?

    convenience init(startingUpdater: Bool = true) {
        let controller = SPUStandardUpdaterController(
            startingUpdater: startingUpdater,
            updaterDelegate: nil,
            userDriverDelegate: nil
        )
        self.init(updater: controller.updater)
    }

    init(updater: SPUUpdater) {
        self.updater = updater
        observation = Publishers.MergeMany([
            updater.publisher(for: \.canCheckForUpdates, options: [.new]),
            updater.publisher(for: \.automaticallyChecksForUpdates, options: [.new]),
            updater.publisher(for: \.automaticallyDownloadsUpdates, options: [.new]),
            updater.publisher(for: \.allowsAutomaticUpdates, options: [.new]),
        ])
        .receive(on: RunLoop.main)
        .sink { [weak self] _ in self?.objectWillChange.send() }
    }

    func checkForUpdates() {
        guard canCheckForUpdates else { return }
        updater.checkForUpdates()
    }

    var automaticallyChecksForUpdates: Bool {
        get { updater.automaticallyChecksForUpdates }
        set { updater.automaticallyChecksForUpdates = newValue }
    }

    var automaticallyDownloadsUpdates: Bool {
        get { updater.automaticallyDownloadsUpdates }
        set { updater.automaticallyDownloadsUpdates = newValue }
    }

    var allowsAutomaticUpdates: Bool {
        updater.allowsAutomaticUpdates
    }

    var canCheckForUpdates: Bool {
        updater.canCheckForUpdates
    }
}
