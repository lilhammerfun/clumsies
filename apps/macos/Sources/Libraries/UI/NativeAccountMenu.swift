import AppKit
import SwiftUI

struct NativeAccountMenu: NSViewRepresentable {
    let account: UserReference?
    let displayName: String
    let onOpenSettings: () -> Void
    let onSignOut: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(configuration: configuration)
    }

    func makeNSView(context: Context) -> NativeAccountMenuButton {
        let button = NativeAccountMenuButton()
        button.target = context.coordinator
        button.action = #selector(Coordinator.presentMenu(_:))
        update(button, coordinator: context.coordinator)
        return button
    }

    func updateNSView(_ nsView: NativeAccountMenuButton, context: Context) {
        update(nsView, coordinator: context.coordinator)
    }

    private var configuration: Configuration {
        Configuration(
            account: account,
            onOpenSettings: onOpenSettings,
            onSignOut: onSignOut
        )
    }

    private func update(_ button: NativeAccountMenuButton, coordinator: Coordinator) {
        coordinator.configuration = configuration
        button.setContent(account: account, displayName: displayName)
        button.setAccessibilityLabel("Account menu for \(displayName)")
        button.setAccessibilityRole(.popUpButton)
    }

    struct Configuration {
        let account: UserReference?
        let onOpenSettings: () -> Void
        let onSignOut: () -> Void
    }

    @MainActor
    final class Coordinator: NSObject {
        var configuration: Configuration

        init(configuration: Configuration) {
            self.configuration = configuration
        }

        @objc func presentMenu(_ sender: NativeAccountMenuButton) {
            let menu = makeMenu()
            menu.popUp(
                positioning: nil,
                at: NSPoint(x: 0, y: sender.bounds.maxY),
                in: sender
            )
        }

        func makeMenu() -> NSMenu {
            let menu = NSMenu()
            menu.autoenablesItems = false

            if let account = configuration.account {
                let identity = NSMenuItem(
                    title: account.email,
                    action: nil,
                    keyEquivalent: ""
                )
                identity.isEnabled = false
                menu.addItem(identity)
            }

            menu.addItem(actionItem("Settings…", action: #selector(openSettings)))
            menu.addItem(.separator())
            menu.addItem(actionItem("Sign Out", action: #selector(signOut)))
            return menu
        }

        private func actionItem(_ title: String, action: Selector) -> NSMenuItem {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self
            item.isEnabled = true
            return item
        }

        @objc private func openSettings() {
            configuration.onOpenSettings()
        }

        @objc private func signOut() {
            configuration.onSignOut()
        }
    }
}

@MainActor
final class NativeAccountMenuButton: NSButton {
    private let hostedLabel = NSHostingView(rootView: AnyView(EmptyView()))

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        configureView()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        configureView()
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: NSView.noIntrinsicMetric, height: 40)
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        bounds.contains(point) ? self : nil
    }

    func setContent(account: UserReference?, displayName: String) {
        hostedLabel.rootView = AnyView(
            NativeAccountMenuLabel(account: account, displayName: displayName)
        )
    }

    private func configureView() {
        title = ""
        isBordered = false
        bezelStyle = .accessoryBarAction
        focusRingType = .none
        setButtonType(.momentaryChange)

        hostedLabel.translatesAutoresizingMaskIntoConstraints = false
        addSubview(hostedLabel)
        NSLayoutConstraint.activate([
            hostedLabel.leadingAnchor.constraint(equalTo: leadingAnchor),
            hostedLabel.trailingAnchor.constraint(equalTo: trailingAnchor),
            hostedLabel.topAnchor.constraint(equalTo: topAnchor),
            hostedLabel.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }
}

private struct NativeAccountMenuLabel: View {
    let account: UserReference?
    let displayName: String

    var body: some View {
        HStack(spacing: 9) {
            UserIdentityLabel(account: account, displayName: displayName)
            Spacer()
        }
        .padding(.horizontal, 10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }
}

/// Shared compact identity treatment for account menus, Claims, comments,
/// and other places that name a real Clumsies member.
struct UserIdentityLabel: View {
    let account: UserReference?
    let displayName: String
    var size: AvatarView.Size = .regular

    var body: some View {
        HStack(spacing: size == .small ? 6 : 9) {
            AvatarView(account: account, size: size)
                .accessibilityHidden(true)
            Text(displayName)
                .lineLimit(1)
        }
        .accessibilityElement(children: .combine)
    }
}
