import AppKit
import Combine
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation {
    private let store = WorkspaceCoordinator()
    private lazy var administration = AdministrationModel(
        context: store.context, onWorkspaceChanged: { [weak store] in await store?.reload() }
    )
    private let softwareUpdateController = SoftwareUpdateController(startingUpdater: NSClassFromString("XCTestCase") == nil)
    let administratorRecoveryState = NativeAdministratorRecoveryState()
    private var phaseObservation: AnyCancellable?
    private var startupTask: Task<Void, Never>?
    private var isChoosingAgents = false
    private var mainWindow: NSWindow?
    private let startupWindowController = StartupWindowController()
    private lazy var reconciliationWindows = ReconciliationWindows(store: store)
    private lazy var settingsWindowController = SettingsWindowController(
        store: store, administration: administration, softwareUpdateController: softwareUpdateController,
        onShowLogs: { [weak self] in self?.showLogsInFinder() },
        onRestart: { [weak self] in self?.restartApplication() }
    )
    private var statusItem: NSStatusItem?
    private lazy var statusMenu = makeStatusMenu()
    private var isFlushingForTermination = false
    private let restartController = AppRestartController()
    private var mainWorkspaceAccountID: String?
    private var mainWorkspaceOrganizationID: String?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Hosted tests must not start live authentication or daemon work.
        guard NSClassFromString("XCTestCase") == nil else { return }
        ClientDiagnostics.record("app_started", ClientDiagnostics.metadata)
        NSApp.setActivationPolicy(.regular)
        installApplicationMenu()
        installStatusItem()
        _ = reconciliationWindows
        observePhase()
        NSApp.activate(ignoringOtherApps: true)
        startupTask = Task { [weak self] in
            await self?.startAfterNativeSetupCheck()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationDidUpdate(_ notification: Notification) {
        for window in NSApp.windows {
            ToolbarHelp.fillMissingTooltips(in: window.toolbar?.items ?? [])
        }
    }

    func applicationShouldHandleReopen(
        _ sender: NSApplication,
        hasVisibleWindows flag: Bool
    ) -> Bool {
        restorePrimaryWindow()
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !isFlushingForTermination else { return .terminateLater }
        guard settingsWindowController.confirmDiscardIfNeeded(),
              reconciliationWindows.closeAllIfAllowed() else {
            _ = finishTermination(allowed: false)
            return .terminateCancel
        }
        guard store.hasPendingChanges else {
            return finishTermination(allowed: true) ? .terminateNow : .terminateCancel
        }
        isFlushingForTermination = true
        Task { [weak self] in
            guard let self else {
                sender.reply(toApplicationShouldTerminate: false)
                return
            }
            let didSave = await store.flushPendingChanges()
            isFlushingForTermination = false
            let canTerminate = finishTermination(allowed: didSave)
            if !canTerminate {
                presentMainWindow()
            }
            sender.reply(toApplicationShouldTerminate: canTerminate)
        }
        return .terminateLater
    }

    private func restartApplication() {
        guard !isFlushingForTermination else { return }
        restartController.request { NSApp.terminate(nil) }
    }

    private func finishTermination(allowed: Bool) -> Bool {
        do {
            return try restartController.finishTermination(allowed: allowed)
        } catch {
            ClientDiagnostics.record("app_restart_failed", ClientDiagnostics.failureFields(error))
            store.feedback.errorMessage = String(localized: "Clumsies could not restart") + ". "
                + String(localized: "Your language choice is saved. Try restarting again, or quit and open Clumsies manually.")
            return false
        }
    }

    func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
        if menuItem.action == #selector(checkForUpdates(_:)) {
            return softwareUpdateController.canCheckForUpdates
        }
        if menuItem.action == #selector(newProject(_:)) {
            return store.context.canCreateProject && store.context.phase == .ready
        }
        if menuItem.action == #selector(newMemory(_:)) {
            guard store.navigation.selectedSection == .memory else { return false }
            return store.edits.canCreateMemory(kind: store.navigation.selectedKind, scope: .org)
        }
        if menuItem.action == #selector(closeActiveTab(_:)) {
            return (NSApp.keyWindow != nil && NSApp.keyWindow !== mainWindow)
                || store.navigation.activeVisibleTab != nil
        }
        if menuItem.action == #selector(toggleSidebar(_:)) {
            menuItem.title = store.navigation.sidebarExpanded ? String(localized: "Hide Sidebar") : String(localized: "Show Sidebar")
        }
        if menuItem.action == #selector(approveReview(_:)) {
            return store.reviews.canPerformReviewMenuAction(.approve)
        }
        if menuItem.action == #selector(rejectReview(_:)) {
            return store.reviews.canPerformReviewMenuAction(.reject)
        }
        if menuItem.action == #selector(mergeReview(_:)) {
            return store.reviews.canPerformReviewMenuAction(.merge)
        }
        if menuItem.action == #selector(resubmitReview(_:)) {
            return store.reviews.canPerformReviewMenuAction(.resubmit)
        }
        return true
    }

    private func observePhase() {
        phaseObservation = store.context.$phase
            .removeDuplicates()
            .receive(on: RunLoop.main)
            .sink { [weak self] phase in
                self?.present(phase)
            }
    }

    private func present(_ phase: ApplicationPhase) {
        switch phase {
        case .launching:
            presentMainLoading()
        case .loading:
            guard !isChoosingAgents else { return }
            if startupWindowController.window?.isVisible == true {
                presentMainLoading()
            } else if mainWindow == nil {
                presentMainLoading()
            }
        case .authenticationRequired:
            isChoosingAgents = false
            mainWindow?.orderOut(nil)
            presentNativeServerAccess(
                purpose: .appSignIn,
                destination: .daemon(store.context.daemon, launchIfNeeded: false)
            ) { [weak self] in
                guard let self else { return }
                Task { await self.store.reload() }
            }
        case .ready:
            guard UserDefaults.standard.bool(forKey: "ClumsiesAgentSetupCompleted") else {
                guard !isChoosingAgents else {
                    startupWindowController.showWindow(nil)
                    return
                }
                isChoosingAgents = true
                presentAuthenticationContent(AgentsSettingsView(model: AgentsSettingsModel(context: self.store.context, integration: self.store.agents)) { [weak self] in
                    UserDefaults.standard.set(true, forKey: "ClumsiesAgentSetupCompleted")
                    guard let self else { return }
                    self.isChoosingAgents = false
                    self.present(self.store.context.phase)
                }.workspaceEnvironment(store))
                return
            }
            let wasAuthenticating = startupWindowController.window != nil
            startupWindowController.window?.orderOut(nil)
            startupWindowController.window = nil
            if mainWindow != nil,
               mainWorkspaceAccountID == store.context.account?.userId,
               mainWorkspaceOrganizationID == store.context.organization?.orgId {
                mainWindow?.title = store.context.organization?.name ?? "Clumsies Lab"
                if wasAuthenticating { mainWindow?.makeKeyAndOrderFront(nil) }
            } else {
                presentMainWindow()
            }
        case .failed(let message):
            isChoosingAgents = false
            mainWindow?.orderOut(nil)
            presentMainFailure(message: message) { [weak store] in
                Task { await store?.reload() }
            }
        }
    }

    private func startAfterNativeSetupCheck() async {
        defer { startupTask = nil }
        guard let origin = try? ServerOrigin(validating: ClumsiesIdentifiers.serverURL) else {
            store.start()
            return
        }
        do {
            let status = try await NativeServerSetupClient(origin: origin).status()
            if status.state == .setupRequired {
                presentNativeServerAccess(
                    purpose: .appSignIn,
                    destination: .daemon(store.context.daemon, launchIfNeeded: true),
                    initialSetupStatus: status
                ) { [weak self] in
                    self?.store.start()
                }
                return
            }
        } catch {
            // Setup detection is deliberately independent of the daemon. If the
            // Server is offline, the daemon still gets a chance to load cached data.
        }
        store.start()
    }

    private func presentNativeServerAccess(
        purpose: NativeServerAccessModel.Purpose,
        destination: NativeServerAccessModel.Destination,
        initialSetupStatus: NativeSetupStatus? = nil,
        onCompleted: @escaping @MainActor () -> Void = {}
    ) {
        let model = NativeServerAccessModel(
            purpose: purpose,
            destination: destination,
            recoveryState: administratorRecoveryState,
            initialSetupStatus: initialSetupStatus,
            onCompleted: onCompleted
        )
        presentAuthenticationContent(NativeServerAccessView(model: model))
    }

    private func presentAdministratorRecovery() {
        presentNativeServerAccess(
            purpose: .administratorRecovery,
            destination: .memoryOnly
        )
    }

    private func presentMainWindow() {
        mainWorkspaceAccountID = store.context.account?.userId
        mainWorkspaceOrganizationID = store.context.organization?.orgId
        presentMainContent(
            WorkspaceView(
                store: store,
                onSignOut: { [weak self] in self?.signOut() },
                onOpenSettings: { [weak self] in self?.presentSettingsWindow() }
            )
            .environmentObject(administration)
            .environmentObject(softwareUpdateController)
            .workspaceEnvironment(store),
            title: store.context.organization?.name ?? "Clumsies Lab"
        )
    }

    private func presentMainLoading() {
        startupWindowController.show(LaunchView(), height: 360)
    }

    private func presentMainFailure(message: String, retry: @escaping () -> Void) {
        presentAuthenticationContent(
            FailureView(
                message: message,
                retry: retry,
                onAdministratorRecovery: { [weak self] in
                    self?.presentAdministratorRecovery()
                },
                onShowLogs: { [weak self] in self?.showLogsInFinder() }
            )
        )
    }

    private func presentMainContent<Content: View>(
        _ content: Content,
        title: String
    ) {
        let contentView = NSHostingView(rootView: content)
        contentView.sizingOptions = []
        if #available(macOS 26.0, *) {
            contentView.sceneBridgingOptions = .all
        }
        if let mainWindow {
            mainWindow.title = title
            mainWindow.contentView = contentView
            mainWindow.makeKeyAndOrderFront(nil)
            return
        }
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1280, height: 820),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = title
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.toolbarStyle = .unified
        window.isMovableByWindowBackground = true
        window.isReleasedWhenClosed = false
        window.contentView = contentView
        window.minSize = NSSize(width: 920, height: 600)
        window.setFrameAutosaveName("ClumsiesNativeWorkspaceWindow")
        if window.frame.width < window.minSize.width || window.frame.height < window.minSize.height {
            window.setContentSize(NSSize(width: 1280, height: 820))
            window.center()
        }
        window.makeKeyAndOrderFront(nil)
        mainWindow = window
    }

    private func presentAuthenticationContent<Content: View>(_ content: Content) {
        startupWindowController.show(content.feedbackHost(showsService: false))
    }

    private func presentSettingsWindow() {
        settingsWindowController.showWindow(nil)
    }

    @objc func showLogsInFinderAction(_ sender: Any?) {
        showLogsInFinder()
    }

    @objc private func exportDiagnostics(_ sender: Any?) {
        DiagnosticsExport.present { [weak self] in self?.store.feedback.errorMessage = $0 }
    }

    private func showLogsInFinder() {
        let path = store.refresh.runtime?.health.logDir
        let logURL: URL
        if let path, !path.isEmpty {
            logURL = URL(fileURLWithPath: path)
        } else {
            logURL = AppBundleRuntimeLocation.defaultLogDirectoryURL
        }
        if FileManager.default.fileExists(atPath: logURL.path) {
            NSWorkspace.shared.activateFileViewerSelecting([logURL])
        } else {
            NSWorkspace.shared.open(logURL)
        }
    }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        guard let button = item.button else { return }
        let image = NSImage(named: "MenuBarIcon")
            ?? NSImage(
                systemSymbolName: "wand.and.stars",
                accessibilityDescription: ClumsiesIdentifiers.appDisplayName
            )
        image?.isTemplate = true
        image?.size = NSSize(width: 18, height: 18)
        button.image = image
        button.imagePosition = .imageOnly
        button.imageScaling = .scaleProportionallyDown
        button.toolTip = ClumsiesIdentifiers.appDisplayName
        button.target = self
        button.action = #selector(handleStatusItemClick(_:))
        button.sendAction(on: [.leftMouseUp, .rightMouseUp])
        statusItem = item
    }

    private func makeStatusMenu() -> NSMenu {
        let menu = NSMenu()
        let open = menu.addItem(
            withTitle: String(localized: "Open \(ClumsiesIdentifiers.appDisplayName)"),
            action: #selector(openFromStatusItem(_:)),
            keyEquivalent: ""
        )
        open.target = self
        let settings = menu.addItem(
            withTitle: String(localized: "Settings..."),
            action: #selector(showSettings(_:)),
            keyEquivalent: ""
        )
        settings.target = self
        let revealLogs = menu.addItem(
            withTitle: String(localized: "Reveal Logs in Finder"),
            action: #selector(showLogsInFinderAction(_:)),
            keyEquivalent: ""
        )
        revealLogs.target = self
        let exportLogs = menu.addItem(withTitle: String(localized: "Export Diagnostics…"), action: #selector(exportDiagnostics(_:)), keyEquivalent: "")
        exportLogs.target = self
        menu.addItem(.separator())
        menu.addItem(
            withTitle: String(localized: "Quit \(ClumsiesIdentifiers.appDisplayName)"),
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: ""
        )
        return menu
    }

    private func restorePrimaryWindow() {
        NSApp.activate(ignoringOtherApps: true)
        if !Self.restoreExistingWindow(startup: startupWindowController.window, workspace: mainWindow) {
            present(store.context.phase)
        }
    }

    static func restoreExistingWindow(startup: NSWindow?, workspace: NSWindow?) -> Bool {
        guard let window = startup ?? workspace else { return false }
        window.deminiaturize(nil)
        window.makeKeyAndOrderFront(nil)
        return true
    }

    @objc private func handleStatusItemClick(_ sender: NSStatusBarButton) {
        if NSApp.currentEvent?.type == .rightMouseUp {
            statusMenu.appearance = NSApp.effectiveAppearance
            statusMenu.popUp(
                positioning: nil,
                at: NSPoint(x: 0, y: sender.bounds.height),
                in: sender
            )
            return
        }
        restorePrimaryWindow()
    }

    @objc private func openFromStatusItem(_ sender: Any?) {
        restorePrimaryWindow()
    }

    private func installApplicationMenu() {
        let mainMenu = NSMenu()
        let applicationItem = NSMenuItem()
        mainMenu.addItem(applicationItem)
        let applicationMenu = NSMenu()
        applicationMenu.addItem(
            withTitle: String(localized: "About \(ClumsiesIdentifiers.appDisplayName)"),
            action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
            keyEquivalent: ""
        )
        applicationMenu.addItem(.separator())
        let settings = applicationMenu.addItem(withTitle: String(localized: "Settings..."), action: #selector(showSettings(_:)), keyEquivalent: ",")
        settings.target = self
        let updates = applicationMenu.addItem(withTitle: String(localized: "Check for Updates..."), action: #selector(checkForUpdates(_:)), keyEquivalent: "")
        updates.target = self
        applicationMenu.addItem(.separator())
        applicationMenu.addItem(
            withTitle: String(localized: "Hide \(ClumsiesIdentifiers.appDisplayName)"),
            action: #selector(NSApplication.hide(_:)),
            keyEquivalent: "h"
        )
        applicationMenu.addItem(withTitle: String(localized: "Hide Others"), action: #selector(NSApplication.hideOtherApplications(_:)), keyEquivalent: "h").keyEquivalentModifierMask = [.command, .option]
        applicationMenu.addItem(.separator())
        applicationMenu.addItem(
            withTitle: String(localized: "Quit \(ClumsiesIdentifiers.appDisplayName)"),
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        applicationItem.submenu = applicationMenu

        let fileItem = NSMenuItem()
        mainMenu.addItem(fileItem)
        let fileMenu = NSMenu(title: String(localized: "File"))
        let newMemory = fileMenu.addItem(withTitle: String(localized: "New Memory"), action: #selector(newMemory(_:)), keyEquivalent: "n")
        newMemory.target = self
        let newProject = fileMenu.addItem(
            withTitle: String(localized: "New Project…"),
            action: #selector(newProject(_:)),
            keyEquivalent: "n"
        )
        newProject.keyEquivalentModifierMask = [.command, .shift]
        newProject.target = self
        fileMenu.addItem(.separator())
        let closeTab = fileMenu.addItem(withTitle: String(localized: "Close Tab"), action: #selector(closeActiveTab(_:)), keyEquivalent: "w")
        closeTab.target = self
        let closeWindow = fileMenu.addItem(
            withTitle: String(localized: "Close Window"),
            action: #selector(NSWindow.performClose(_:)),
            keyEquivalent: "w"
        )
        closeWindow.keyEquivalentModifierMask = [.command, .shift]
        fileItem.submenu = fileMenu

        let editItem = NSMenuItem()
        mainMenu.addItem(editItem)
        let editMenu = NSMenu(title: String(localized: "Edit"))
        editMenu.addItem(withTitle: String(localized: "Undo"), action: Selector(("undo:")), keyEquivalent: "z")
        editMenu.addItem(withTitle: String(localized: "Redo"), action: Selector(("redo:")), keyEquivalent: "Z")
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: String(localized: "Cut"), action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: String(localized: "Copy"), action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: String(localized: "Paste"), action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: String(localized: "Select All"), action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editMenu.addItem(.separator())
        let find = editMenu.addItem(
            withTitle: String(localized: "Find..."),
            action: #selector(NSTextView.performFindPanelAction(_:)),
            keyEquivalent: "f"
        )
        find.tag = 1
        let findNext = editMenu.addItem(
            withTitle: String(localized: "Find Next"),
            action: #selector(NSTextView.performFindPanelAction(_:)),
            keyEquivalent: "g"
        )
        findNext.tag = 2
        let findPrevious = editMenu.addItem(
            withTitle: String(localized: "Find Previous"),
            action: #selector(NSTextView.performFindPanelAction(_:)),
            keyEquivalent: "g"
        )
        findPrevious.tag = 3
        findPrevious.keyEquivalentModifierMask = [.command, .shift]
        editMenu.addItem(.separator())
        let search = editMenu.addItem(withTitle: String(localized: "Search Clumsies..."), action: #selector(showSearch(_:)), keyEquivalent: "k")
        search.target = self
        editItem.submenu = editMenu

        let viewItem = NSMenuItem()
        mainMenu.addItem(viewItem)
        let viewMenu = NSMenu(title: String(localized: "View"))
        let sidebar = viewMenu.addItem(withTitle: String(localized: "Toggle Sidebar"), action: #selector(toggleSidebar(_:)), keyEquivalent: "s")
        sidebar.keyEquivalentModifierMask = [.command, .option]
        sidebar.target = self
        viewItem.submenu = viewMenu

        let reviewItem = NSMenuItem()
        mainMenu.addItem(reviewItem)
        let reviewMenu = NSMenu(title: String(localized: "Review"))
        let approve = reviewMenu.addItem(
            withTitle: String(localized: "Approve"),
            action: #selector(approveReview(_:)),
            keyEquivalent: "a"
        )
        approve.keyEquivalentModifierMask = [.command, .option]
        approve.target = self
        let reject = reviewMenu.addItem(
            withTitle: String(localized: "Reject"),
            action: #selector(rejectReview(_:)),
            keyEquivalent: "r"
        )
        reject.keyEquivalentModifierMask = [.command, .option]
        reject.target = self
        reviewMenu.addItem(.separator())
        let merge = reviewMenu.addItem(
            withTitle: String(localized: "Merge"),
            action: #selector(mergeReview(_:)),
            keyEquivalent: "m"
        )
        merge.keyEquivalentModifierMask = [.command, .option]
        merge.target = self
        let resubmit = reviewMenu.addItem(
            withTitle: String(localized: "Resubmit"),
            action: #selector(resubmitReview(_:)),
            keyEquivalent: "u"
        )
        resubmit.keyEquivalentModifierMask = [.command, .option]
        resubmit.target = self
        reviewItem.submenu = reviewMenu

        let windowItem = NSMenuItem()
        mainMenu.addItem(windowItem)
        let windowMenu = NSMenu(title: String(localized: "Window"))
        windowMenu.addItem(withTitle: String(localized: "Minimize"), action: #selector(NSWindow.miniaturize(_:)), keyEquivalent: "m")
        windowMenu.addItem(withTitle: String(localized: "Zoom"), action: #selector(NSWindow.performZoom(_:)), keyEquivalent: "")
        windowMenu.addItem(.separator())
        windowMenu.addItem(withTitle: String(localized: "Bring All to Front"), action: #selector(NSApplication.arrangeInFront(_:)), keyEquivalent: "")
        windowItem.submenu = windowMenu
        NSApp.windowsMenu = windowMenu
        NSApp.mainMenu = mainMenu
    }

    private func signOut() {
        guard settingsWindowController.confirmDiscardIfNeeded() else { return }
        guard reconciliationWindows.closeAllIfAllowed() else { return }
        Task { await store.signOut() }
    }

    @objc private func showSettings(_ sender: Any?) {
        NSApp.activate(ignoringOtherApps: true)
        presentSettingsWindow()
    }

    @objc private func newMemory(_ sender: Any?) {
        guard store.navigation.selectedSection == .memory else { return }
        Task { await store.memory.createMemory(kind: store.navigation.selectedKind, scope: .org) }
    }

    @objc private func newProject(_ sender: Any?) {
        store.navigation.presentProjectCreation()
    }

    @objc private func closeActiveTab(_ sender: Any?) {
        if let keyWindow = NSApp.keyWindow, keyWindow !== mainWindow {
            keyWindow.performClose(sender)
            return
        }
        if !store.navigation.closeActiveTab() {
            mainWindow?.performClose(sender)
        }
    }

    @objc private func checkForUpdates(_ sender: Any?) {
        softwareUpdateController.checkForUpdates()
    }

    @objc private func showSearch(_ sender: Any?) {
        mainWindow?.makeKeyAndOrderFront(nil)
        switch store.navigation.selectedSection {
        case .reviews:
            store.navigation.focusReviewSearch()
        case .memory, .bundles, .inbox:
            store.navigation.focusWorkspaceSearch()
        case .sessions, .dashboard:
            break
        }
    }

    @objc private func toggleSidebar(_ sender: Any?) {
        store.navigation.sidebarExpanded.toggle()
        mainWindow?.makeKeyAndOrderFront(nil)
    }

    @objc private func approveReview(_ sender: Any?) {
        Task { await store.reviews.performReviewMenuAction(.approve) }
    }

    @objc private func rejectReview(_ sender: Any?) {
        Task { await store.reviews.performReviewMenuAction(.reject) }
    }

    @objc private func mergeReview(_ sender: Any?) {
        Task { await store.reviews.performReviewMenuAction(.merge) }
    }

    @objc private func resubmitReview(_ sender: Any?) {
        Task { await store.reviews.performReviewMenuAction(.resubmit) }
    }
}
