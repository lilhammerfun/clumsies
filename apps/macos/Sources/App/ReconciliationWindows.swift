import AppKit
import Combine
import SwiftUI

/// Owns document reconciliation windows independently of document tabs.
@MainActor
final class ReconciliationWindows {
    private let store: WorkspaceCoordinator
    private var observations: Set<AnyCancellable> = []
    private(set) var documentWindows: [MemoryDocumentSessionKey: ReconciliationWindowController] = [:]

    init(store: WorkspaceCoordinator) {
        self.store = store
        store.sessions.$pendingDocumentReconciliationCandidatesBySession
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateDocumentWindows() }
            .store(in: &observations)
    }

    /// Closing the app must use the same unsaved-edit and in-flight-save guards.
    func closeAllIfAllowed() -> Bool {
        let pendingReviews = store.reviews.updates.values
        guard !pendingReviews.contains(where: \.isApplying) else {
            NSSound.beep()
            return false
        }
        if pendingReviews.contains(where: \.hasEdits) {
            let alert = NSAlert()
            alert.messageText = "Discard unsaved Review choices?"
            alert.informativeText = "Choose Review Actions (…) > Save Review Updates to keep your choices."
            alert.addButton(withTitle: "Keep Editing")
            alert.addButton(withTitle: "Discard Edits")
            guard alert.runModal() == .alertSecondButtonReturn else { return false }
        }
        let windows = Array(documentWindows.values)
        guard windows.allSatisfy({ $0.confirmCloseIfNeeded() }) else { return false }
        for controller in windows { controller.window?.close() }
        return true
    }

    private func updateDocumentWindows() {
        let candidates = store.sessions.pendingDocumentReconciliationCandidatesBySession
        for (key, controller) in documentWindows where candidates[key]?.candidateId != controller.identity {
            controller.dismiss()
            documentWindows.removeValue(forKey: key)
        }
        for (key, candidate) in candidates where documentWindows[key] == nil {
            let sessions = store.sessions
            let path = candidate.draftState.resource.path ?? candidate.currentState.resource.path ?? "Untitled"
            let controller = ReconciliationWindowController(
                identity: candidate.candidateId,
                title: "\((path as NSString).lastPathComponent) — \(candidate.status == .conflicts ? "Resolve Conflicts" : "Update Draft")",
                subtitle: path,
                autosaveName: "ClumsiesDraftReconciliationWindow",
                hasEdits: { sessions.documentReconciliationResolutions[key]?.hasEdits == true },
                isSaving: { sessions.applyingDocumentReconciliationSessions.contains(key) },
                onClose: { sessions.finishDocumentReconciliation(for: key) }
            )
            controller.install(DraftReconciliationView(
                candidate: candidate,
                initialResolution: sessions.documentReconciliationResolutions[key],
                onResolutionChange: { resolution in
                    guard sessions.pendingDocumentReconciliationCandidatesBySession[key]?.candidateId == candidate.candidateId else { return }
                    sessions.documentReconciliationResolutions[key] = resolution
                },
                onCancel: { sessions.finishDocumentReconciliation(for: key) }
            ) { [reconciler = store.reconciliation] resolvedState in
                try await reconciler.applyReconciliation(
                    draftId: candidate.draftId, candidate: candidate, resolvedState: resolvedState,
                    projectId: key.projectId, documentItemId: key.itemId
                )
            })
            documentWindows[key] = controller
            controller.showWindow(nil)
        }
    }
}

@MainActor
final class ReconciliationWindowController: NSWindowController, NSWindowDelegate {
    let identity: String
    private let hasEdits: () -> Bool
    private let isSaving: () -> Bool
    private var onClose: (() -> Void)?
    var confirmDiscard: () -> Bool = {
        let alert = NSAlert()
        alert.messageText = "Discard unsaved resolution edits?"
        alert.informativeText = "Your draft has not been changed. Keep editing to save this result, or discard these edits."
        alert.addButton(withTitle: "Keep Editing")
        alert.addButton(withTitle: "Discard Edits")
        return alert.runModal() == .alertSecondButtonReturn
    }

    init(identity: String, title: String, subtitle: String, autosaveName: String,
         hasEdits: @escaping () -> Bool, isSaving: @escaping () -> Bool,
         onClose: @escaping () -> Void) {
        self.identity = identity
        self.hasEdits = hasEdits
        self.isSaving = isSaving
        self.onClose = onClose
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1200, height: 800),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false
        )
        window.title = title
        window.subtitle = subtitle
        window.toolbar = NSToolbar(identifier: "ReconciliationToolbar")
        window.toolbarStyle = .unified
        window.contentMinSize = NSSize(width: 900, height: 600)
        window.collectionBehavior.insert(.fullScreenPrimary)
        window.tabbingMode = .disallowed
        window.isReleasedWhenClosed = false
        window.center()
        window.setFrameAutosaveName(autosaveName)
        super.init(window: window)
        window.delegate = self
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func install<Content: View>(_ content: Content) {
        guard let window else { return }
        let frame = window.frame
        let host = NSHostingController(rootView: content)
        host.sizingOptions = []
        window.contentViewController = host
        // AppKit initially fits a hosting controller to the view's minimum size.
        window.setFrame(frame, display: false)
    }

    override func showWindow(_ sender: Any?) {
        window?.deminiaturize(sender)
        super.showWindow(sender)
        window?.makeKeyAndOrderFront(sender)
    }

    func confirmCloseIfNeeded() -> Bool {
        guard !isSaving() else {
            showWindow(nil)
            NSSound.beep()
            return false
        }
        guard hasEdits() else { return true }
        showWindow(nil)
        return confirmDiscard()
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool { confirmCloseIfNeeded() }

    func windowWillClose(_ notification: Notification) {
        let completion = onClose
        onClose = nil
        completion?()
    }

    /// The owning session ended or was invalidated; do not cancel another session.
    func dismiss() {
        onClose = nil
        close()
        window?.contentViewController = nil
    }
}
