import Combine
import Sparkle

@MainActor
final class SoftwareUpdateController: NSObject, ObservableObject {
    @Published private(set) var hasAvailableUpdate = false

    private let injectedUpdater: SPUUpdater?
    private lazy var controller = SPUStandardUpdaterController(
        startingUpdater: false,
        updaterDelegate: self,
        userDriverDelegate: self
    )
    private var updater: SPUUpdater { injectedUpdater ?? controller.updater }
    private var observation: AnyCancellable?

    init(startingUpdater: Bool = true) {
        injectedUpdater = nil
        super.init()
        observeUpdater()
        if startingUpdater {
            controller.startUpdater()
        }
    }

    init(updater: SPUUpdater) {
        injectedUpdater = updater
        super.init()
        observeUpdater()
    }

    private func observeUpdater() {
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

extension SoftwareUpdateController: SPUUpdaterDelegate, @preconcurrency SPUStandardUserDriverDelegate {
    var supportsGentleScheduledUpdateReminders: Bool { true }

    func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        hasAvailableUpdate = true
    }

    func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck, error: Error?) {
        hasAvailableUpdate = false
    }

    func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool, forUpdate update: SUAppcastItem, state: SPUUserUpdateState) {
        // Restored downloads can be presented without fetching the appcast again.
        hasAvailableUpdate = true
    }

    func standardUserDriverWillFinishUpdateSession() {
        hasAvailableUpdate = false
    }
}
