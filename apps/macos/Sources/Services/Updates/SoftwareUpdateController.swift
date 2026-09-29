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
    private var probeTask: Task<Void, Never>?
    private var isProbing = false
    private var preferences: UserDefaults {
        UserDefaults(suiteName: updater.hostBundle.bundleIdentifier ?? "") ?? .standard
    }

    deinit { probeTask?.cancel() }

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
        ])
        .receive(on: RunLoop.main)
        .sink { [weak self] _ in
            self?.objectWillChange.send()
            self?.startProbeScheduler()
        }
    }

    private func startProbeScheduler() {
        guard injectedUpdater == nil, probeTask == nil, canCheckForUpdates else { return }
        probeTask = Task { [weak self] in
            while !Task.isCancelled {
                self?.probeIfEnabled()
                try? await Task.sleep(for: .seconds(3600))
            }
        }
    }

    private func probeIfEnabled() {
        guard automaticallyChecksForUpdates else { return }
        checkForUpdateInformation()
    }

    func checkForUpdateInformation() {
        guard canCheckForUpdates, !updater.sessionInProgress, !isProbing else { return }
        isProbing = true
        updater.checkForUpdateInformation()
    }

    func checkForUpdates() {
        Self.log.info("Update action requested; can_check=\(self.canCheckForUpdates)")
        guard canCheckForUpdates else { return }
        updater.checkForUpdates()
    }

    var automaticallyChecksForUpdates: Bool {
        get { preferences.object(forKey: "SUEnableAutomaticChecks") as? Bool ?? false }
        set {
            preferences.set(newValue, forKey: "SUEnableAutomaticChecks")
            objectWillChange.send()
            if newValue { probeIfEnabled() }
            else { hasAvailableUpdate = false }
        }
    }

    var canCheckForUpdates: Bool {
        updater.canCheckForUpdates
    }
}

extension SoftwareUpdateController: SPUUpdaterDelegate, @preconcurrency SPUStandardUserDriverDelegate {
    var supportsGentleScheduledUpdateReminders: Bool { true }

    func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        if isProbing, automaticallyChecksForUpdates { hasAvailableUpdate = true }
        Self.log.info("Update found; can_check=\(self.canCheckForUpdates)")
    }

    func updaterDidNotFindUpdate(_ updater: SPUUpdater, error: Error) {
        if isProbing { hasAvailableUpdate = false }
    }

    func updater(_ updater: SPUUpdater, willInstallUpdateOnQuit item: SUAppcastItem, immediateInstallationBlock: @escaping () -> Void) -> Bool {
        preparedToInstallOnQuit = true
        Self.log.info("Update prepared for installation on quit")
        // Keep Sparkle's scheduler and install-on-quit behavior in control.
        return false
    }

    func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck, error: Error?) {
        if isProbing {
            isProbing = false
            return
        }
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
