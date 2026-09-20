import SwiftUI

struct NativeServerAccessView: View {
    @ObservedObject var model: NativeServerAccessModel

    private let brandAccent = Color(red: 0.78, green: 0.24, blue: 0.52)

    var body: some View {
        ScrollView {
            VStack(spacing: 20) {
                BrandLogoView(size: 68, isBreathing: model.isBusy)

                VStack(spacing: 7) {
                    Text(model.title)
                        .font(.system(size: 22, weight: .bold, design: .rounded))
                    Text(model.subtitle)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 390)
                }

                if model.recoveryReady {
                    recoveryContent
                } else {
                    serverField
                    if model.showsSetup {
                        setupFields
                    }
                    primaryAction
                }

                if let message = model.errorMessage {
                    Text(message)
                        .font(.callout)
                        .foregroundStyle(.red)
                        .multilineTextAlignment(.center)
                        .textSelection(.enabled)
                        .frame(maxWidth: 410)
                }
            }
            .padding(36)
            .frame(maxWidth: .infinity)
        }
        .background(Color(nsColor: .textBackgroundColor))
    }

    private var serverField: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Server address")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            TextField("https://clumsies.example.com", text: $model.serverOrigin)
                .textFieldStyle(.roundedBorder)
                .disabled(model.isBusy)
            Text("Remote Servers require HTTPS. HTTP is accepted only on this Mac's loopback address.")
                .font(.caption2)
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: 410)
    }

    private var setupFields: some View {
        VStack(alignment: .leading, spacing: 13) {
            if !model.setupCodeConfigured {
                setupWarning(String(localized: "Set CLUMSIES_SETUP_CODE in the Server deployment before continuing."))
            }
            if !model.oidcConfigured {
                setupWarning(String(localized: "Configure the Server's OIDC deployment settings before continuing."))
            }
            labeledSecureField(String(localized: "Setup code"), placeholder: String(localized: "Deployment setup code"), text: $model.setupCode)
            labeledField(String(localized: "Organization"), placeholder: "Acme", text: $model.organizationName)
            labeledField(String(localized: "Default project"), placeholder: String(localized: "Default"), text: $model.defaultProjectName)
            labeledField(
                String(localized: "Allowed email domains (optional)"),
                placeholder: "example.com, subsidiary.example",
                text: $model.allowedEmailDomains
            )
        }
        .frame(maxWidth: 410)
    }

    private var primaryAction: some View {
        VStack(spacing: 10) {
            Button {
                if model.showsSetup {
                    model.completeSetup()
                } else {
                    model.continueFromServer()
                }
            } label: {
                HStack(spacing: 8) {
                    if model.isBusy {
                        ProgressView().controlSize(.small)
                    }
                    Text(model.showsSetup ? "Save and Continue in Browser" : "Continue in Browser")
                        .fontWeight(.medium)
                }
                .frame(minWidth: 230)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .tint(brandAccent)
            .disabled(
                model.isBusy
                    || (model.showsSetup
                        && (!model.setupCodeConfigured || !model.oidcConfigured))
            )

            if model.showsSetup {
                Button("Use a different Server") { model.chooseAnotherServer() }
                    .buttonStyle(.link)
                    .disabled(model.isBusy)
            }
        }
    }

    private var recoveryContent: some View {
        NativeAdministratorRecoveryPanel(
            state: model.recoveryState,
            identity: model.recoveryIdentity
        )
    }

    private func labeledField(_ title: String, placeholder: String, text: Binding<String>)
        -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            TextField(placeholder, text: text)
                .textFieldStyle(.roundedBorder)
                .disabled(model.isBusy)
        }
    }

    private func labeledSecureField(
        _ title: String,
        placeholder: String,
        text: Binding<String>
    ) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            SecureField(placeholder, text: text)
                .textFieldStyle(.roundedBorder)
                .disabled(model.isBusy)
        }
    }

    private func setupWarning(_ message: String) -> some View {
        Label(message, systemImage: "exclamationmark.triangle.fill")
            .font(.caption)
            .foregroundStyle(.orange)
    }
}

private struct NativeAdministratorRecoveryPanel: View {
    @ObservedObject var state: NativeAdministratorRecoveryState
    let identity: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 10) {
                Image(systemName: "checkmark.shield.fill")
                    .font(.system(size: 28))
                    .foregroundStyle(.green)
                VStack(alignment: .leading, spacing: 2) {
                    Text(identity ?? String(localized: "Administrator authenticated"))
                        .font(.callout.weight(.semibold))
                        .textSelection(.enabled)
                    Text("Direct Server recovery · token held only in App memory")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button {
                    Task { await state.load() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .disabled(state.isLoading || state.mutatingID != nil)
                .help("Refresh recovery data")
            }

            if state.isLoading && state.health == nil {
                HStack {
                    Spacer()
                    ProgressView("Loading Server recovery data…")
                    Spacer()
                }
                .padding(.vertical, 18)
            } else {
                if let health = state.health {
                    NativeRecoveryHealthSection(health: health)
                }
                NativeRecoveryMembersSection(state: state)
                NativeRecoveryTokensSection(state: state)
            }

            if let errorMessage = state.errorMessage {
                Text(errorMessage)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: 430, alignment: .leading)
        .task { await state.load() }
    }
}

private struct NativeRecoveryHealthSection: View {
    let health: AdminHealthRecord

    private var checks: [(String, AdminHealthCheck)] {
        [
            (String(localized: "Database"), health.database),
            (String(localized: "Schema"), health.schema),
            (String(localized: "Commit service"), health.commitService),
            ("OIDC", health.oidc),
        ]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack {
                Text("Server health").font(.headline)
                Spacer()
                Text("\(health.status.title) · \(health.version)")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(health.status == .ok ? Color.green : Color.orange)
            }
            ForEach(checks, id: \.0) { name, check in
                HStack(spacing: 7) {
                    Circle()
                        .fill(check.status == .ok ? Color.green : Color.orange)
                        .frame(width: 7, height: 7)
                    Text(name).font(.caption.weight(.medium))
                    Spacer()
                    Text(check.message)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        }
        .padding(12)
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}

private struct NativeRecoveryMembersSection: View {
    @ObservedObject var state: NativeAdministratorRecoveryState

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Member access").font(.headline)
            if state.members.isEmpty {
                Text("No members returned.").font(.caption).foregroundStyle(.secondary)
            } else {
                ForEach(state.members) { member in
                    NativeRecoveryMemberRow(state: state, member: member)
                    if member.id != state.members.last?.id { Divider() }
                }
            }
        }
    }
}

private struct NativeRecoveryMemberRow: View {
    @ObservedObject var state: NativeAdministratorRecoveryState
    let member: AdminOrganizationMemberRecord

    private var isCurrentUser: Bool { member.id == state.currentUserID }
    private var isMutating: Bool { state.mutatingID == member.id }

    var body: some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(member.displayName ?? member.email)
                    .font(.caption.weight(.medium))
                    .lineLimit(1)
                Text(member.email)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            Menu(member.role.title) {
                ForEach(AdminOrganizationRole.allCases) { role in
                    Button(role.title) {
                        Task { await state.setRole(role, for: member) }
                    }
                    .disabled(role == member.role)
                }
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .disabled(isCurrentUser || state.mutatingID != nil)

            Button(member.status == .disabled ? "Enable" : "Disable") {
                Task { await state.setDisabled(member.status != .disabled, for: member) }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(isCurrentUser || state.mutatingID != nil)
            .overlay {
                if isMutating { ProgressView().controlSize(.mini) }
            }
        }
    }
}

private struct NativeRecoveryTokensSection: View {
    @ObservedObject var state: NativeAdministratorRecoveryState
    @State private var pendingRevocation: AdminAccessTokenRecord?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Access tokens").font(.headline)
            if state.tokens.isEmpty {
                Text("No active tokens returned.").font(.caption).foregroundStyle(.secondary)
            } else {
                ForEach(state.tokens) { token in
                    HStack(spacing: 8) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(token.kind.title)
                                .font(.caption.weight(.medium))
                            Text(token.userId)
                                .font(.caption2.monospaced())
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }
                        Spacer()
                        Button(token.revoked ? "Revoked" : "Revoke") {
                            pendingRevocation = token
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(token.revoked || state.mutatingID != nil)
                    }
                    if token.id != state.tokens.last?.id { Divider() }
                }
            }
        }
        .confirmationDialog(
            "Revoke access token?",
            isPresented: Binding(
                get: { pendingRevocation != nil },
                set: { if !$0 { pendingRevocation = nil } }
            ),
            presenting: pendingRevocation
        ) { token in
            Button("Revoke \(token.kind.title) token", role: .destructive) {
                pendingRevocation = nil
                Task { await state.revoke(token) }
            }
            Button("Cancel", role: .cancel) {
                pendingRevocation = nil
            }
        } message: { token in
            Text("Token \(token.id) will stop working immediately. This cannot be undone.")
        }
    }
}
