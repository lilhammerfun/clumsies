import AppKit
import SwiftUI

extension View {
    /// Independent windows share a foreground-only loop; feature loaders own data and edit guards.
    func automaticRefresh<ID: Hashable>(id: ID, initially: Bool = true,
                                       action: @escaping @MainActor () async -> Void) -> some View {
        modifier(AutomaticRefresh(id: id, initially: initially, action: action))
    }
}

private struct AutomaticRefresh<ID: Hashable>: ViewModifier {
    let id: ID
    let initially: Bool
    let action: @MainActor () async -> Void
    @State private var active = NSApplication.shared.isActive
    @State private var activation = 0

    private struct Input: Hashable {
        let id: ID
        let active: Bool
        let activation: Int
    }

    func body(content: Content) -> some View {
        content
            .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
                active = true
                activation += 1
            }
            .onReceive(NotificationCenter.default.publisher(for: NSApplication.didResignActiveNotification)) { _ in
                active = false
            }
            .onReceive(NSWorkspace.shared.notificationCenter.publisher(for: NSWorkspace.didWakeNotification)) { _ in
                activation += 1
            }
            .task(id: Input(id: id, active: active, activation: activation)) {
                guard active else { return }
                if initially || activation > 0 { await action() }
                while !Task.isCancelled {
                    do { try await Task.sleep(for: .seconds(30)) } catch { return }
                    guard !Task.isCancelled else { return }
                    await action()
                }
            }
    }
}
