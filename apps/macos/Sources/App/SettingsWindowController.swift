import AppKit
import Combine
import SwiftUI

@MainActor
enum SettingsWindowLayout {
    static let defaultContentSize = NSSize(width: 760, height: 720)
    static let minimumContentSize = NSSize(width: 700, height: 560)

    static func normalize(_ window: NSWindow) {
        window.styleMask.formUnion([.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView])
        window.titlebarAppearsTransparent = true
        window.toolbarStyle = .unified
        window.contentMinSize = minimumContentSize
        if window.contentLayoutRect.width < minimumContentSize.width
            || window.contentLayoutRect.height < minimumContentSize.height {
            window.setContentSize(defaultContentSize)
        }
    }
}

@MainActor
final class SettingsWindowController: NSWindowController, NSWindowDelegate {
    let navigation: SettingsNavigation
    private let store: WorkspaceCoordinator
    private let administration: AdministrationModel
    private let softwareUpdateController: SoftwareUpdateController
    private let onShowLogs: () -> Void
    private var authorityObservation: AnyCancellable?
    private var organizationID: String?
    private var accountID: String?
    private var discardsOnClose = false
    private var hadOrganizationAccess: Bool
    private var mutationObservation: AnyCancellable?
    var confirmDiscard: () -> Bool = {
        let alert = NSAlert()
        alert.messageText = String(localized: "Discard unsaved changes?")
        alert.informativeText = String(localized: "Keep editing to save your changes, or discard them.")
        alert.addButton(withTitle: String(localized: "Keep Editing"))
        alert.addButton(withTitle: String(localized: "Discard Changes"))
        return alert.runModal() == .alertSecondButtonReturn
    }

    init(store: WorkspaceCoordinator, administration: AdministrationModel, softwareUpdateController: SoftwareUpdateController,
         onShowLogs: @escaping () -> Void,
         navigation: SettingsNavigation = SettingsNavigation()) {
        self.store = store
        self.administration = administration
        self.softwareUpdateController = softwareUpdateController
        self.onShowLogs = onShowLogs
        self.navigation = navigation
        organizationID = store.context.organization?.orgId
        accountID = store.context.account?.userId
        hadOrganizationAccess = store.context.canAdministerOrganization && store.context.phase != .authenticationRequired
        super.init(window: nil)
        mutationObservation = store.context.$isMutatingAdministration
            .sink { [weak self] saving in self?.navigation.isSaving = saving }
        authorityObservation = store.context.$organization
            .combineLatest(store.context.$account, store.context.$capabilities, store.context.$phase)
            .receive(on: RunLoop.main)
            .sink { [weak self] organization, account, capabilities, phase in
                guard let self else { return }
                let identityChanged = organization?.orgId != self.organizationID || account?.userId != self.accountID
                self.organizationID = organization?.orgId
                self.accountID = account?.userId
                let hasAccess = capabilities.contains("admin:write") && phase != .authenticationRequired
                if identityChanged || (self.hadOrganizationAccess && !hasAccess)
                    || (self.navigation.destination.pane == .organization && !hasAccess) {
                    self.navigation.resetForAuthorityChange()
                }
                self.hadOrganizationAccess = hasAccess
            }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func showWindow(_ sender: Any?) {
        if navigation.destination.pane == .organization,
           !store.context.canAdministerOrganization || store.context.phase == .authenticationRequired {
            navigation.resetForAuthorityChange()
        }
        if window == nil {
            let host = NSHostingController(rootView: SettingsWindowView(softwareUpdateController: softwareUpdateController, navigation: navigation,
                onShowLogs: onShowLogs
            ).environmentObject(administration).workspaceEnvironment(store))
            host.sizingOptions = []
            host.sceneBridgingOptions = .all
            let window = NSWindow(
                contentRect: NSRect(origin: .zero, size: SettingsWindowLayout.defaultContentSize),
                styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                backing: .buffered, defer: false
            )
            window.contentViewController = host
            window.isReleasedWhenClosed = false
            window.delegate = self
            SettingsWindowLayout.normalize(window)
            window.center()
            window.setFrameAutosaveName("ClumsiesSettingsWindow")
            self.window = window
        }
        if let window {
            SettingsWindowLayout.normalize(window)
            window.deminiaturize(sender)
        }
        super.showWindow(sender)
        window?.makeKeyAndOrderFront(sender)
    }

    func confirmDiscardIfNeeded() -> Bool {
        guard !store.context.isMutatingAdministration else {
            window?.makeKeyAndOrderFront(nil)
            let alert = NSAlert()
            alert.messageText = String(localized: "Changes are still being saved")
            alert.informativeText = String(localized: "Wait for saving to finish before signing out or quitting.")
            alert.addButton(withTitle: String(localized: "OK"))
            alert.runModal()
            return false
        }
        return !navigation.hasUnsavedChanges || confirmDiscard()
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if store.context.isMutatingAdministration && navigation.hasUnsavedChanges { return false }
        guard !navigation.hasUnsavedChanges || confirmDiscard() else { return false }
        discardsOnClose = navigation.hasUnsavedChanges
        return true
    }

    func windowWillClose(_ notification: Notification) {
        guard discardsOnClose else { return }
        window?.contentViewController = nil
        window = nil
        navigation.resetForAuthorityChange()
        discardsOnClose = false
    }
}
