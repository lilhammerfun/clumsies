import Combine
import OSLog
import Sparkle

@MainActor
final class SoftwareUpdateController: NSObject, ObservableObject {
    @Published private(set) var hasAvailableUpdate = false

    private static let log = Logger(subsystem: ClumsiesIdentifiers.namespace, category: "SoftwareUpdate")
    private let injectedUpdater: SPUUpdater?
    private lazy var controller = SPUStandardUpdaterController(
        startingUpdater: false,
        updaterDelegate: self,
        userDriverDelegate: self
    )
    private var updater: SPUUpdater { injectedUpdater ?? controller.updater }
    private var observation: AnyCancellable?
    private var preparedToInstallOnQuit = false

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
        Self.log.info("Update action requested; can_check=\(self.canCheckForUpdates)")
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
        Self.log.info("Update found; can_check=\(self.canCheckForUpdates)")
    }

    func updater(_ updater: SPUUpdater, willInstallUpdateOnQuit item: SUAppcastItem, immediateInstallationBlock: @escaping () -> Void) -> Bool {
        preparedToInstallOnQuit = true
        Self.log.info("Update prepared for installation on quit")
        // Keep Sparkle's scheduler and install-on-quit behavior in control.
        return false
    }

    func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck, error: Error?) {
        // Sparkle releases the background session before this callback, so its prepared
        // installation can now be resumed by checkForUpdates without starting a download.
        hasAvailableUpdate = preparedToInstallOnQuit && error == nil
        preparedToInstallOnQuit = false
        let errorCode = (error as NSError?)?.code ?? 0
        Self.log.info("Update cycle finished; kind=\(updateCheck.rawValue), failed=\(error != nil), error_code=\(errorCode), reminder=\(self.hasAvailableUpdate), can_check=\(self.canCheckForUpdates)")
    }

    func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool, forUpdate update: SUAppcastItem, state: SPUUserUpdateState) {
        // Finding a version is too early: automatic downloads cannot be brought into focus.
        // This callback also covers restored downloads that do not fetch the appcast again.
        hasAvailableUpdate = true
        Self.log.info("Update presentation ready; stage=\(state.stage.rawValue), user_initiated=\(state.userInitiated), reminder=true")
    }

    func standardUserDriverWillFinishUpdateSession() {
        hasAvailableUpdate = false
        Self.log.info("Update presentation ended; reminder=false")
    }
}
