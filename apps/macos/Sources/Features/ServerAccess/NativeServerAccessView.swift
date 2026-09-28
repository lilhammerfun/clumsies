import SwiftUI

struct NativeServerAccessView: View {
    @ObservedObject var model: NativeServerAccessModel

    @State private var serverExpanded = false
    @FocusState private var focusedField: Field?
    private enum Field { case username, password, confirmation, credential, server }

    var body: some View {
        ScrollView {
            VStack(spacing: 12) {
                HStack(spacing: 6) {
                    BrandLogoView(size: 40, isBreathing: model.isBusy)
                    Text(model.title).font(.system(size: 20, weight: .semibold))
                }
                .padding(.bottom, 4)
                if model.recoveryReady {
                    recoveryContent
                } else {
                    if model.showsSetup {
                        setupFields
                        if model.loginMethods?.passwordEnabled == true && model.oidcConfigured {
                            Toggle("Set up with username and password", isOn: $model.setupWithPassword)
                        }
                        if model.setupWithPassword { localFields }
                        Button(model.setupWithPassword ? "Create owner" : "Continue with identity provider") { model.completeSetup() }
                            .buttonStyle(SignInButtonStyle(primary: true)).disabled(model.isBusy || !model.serverReady || !model.setupCodeConfigured)
                    } else {
                        if model.loginMethods?.passwordEnabled == true {
                            localFields
                            Button(model.localAction == .signIn ? "Sign in" : model.localAction == .invitation ? "Accept invitation" : "Reset password") { model.signInWithPassword() }
                                .buttonStyle(SignInButtonStyle(primary: true))
                                .disabled(model.isBusy || !model.serverReady || model.password.isEmpty)
                                .keyboardShortcut(.defaultAction)
                        }
                        if model.localAction == .signIn && model.loginMethods?.oidcEnabled == true {
                            if model.loginMethods?.passwordEnabled == true {
                                HStack(spacing: 12) {
                                    Rectangle().fill(Color(nsColor: .separatorColor)).frame(height: 0.5)
                                    Text("or").font(.system(size: 11)).foregroundStyle(.secondary)
                                    Rectangle().fill(Color(nsColor: .separatorColor)).frame(height: 0.5)
                                }.padding(.vertical, 1)
                            }
                            Button { model.continueFromServer() } label: {
                                HStack(spacing: 12) {
                                    if model.loginMethods?.google == true {
                                        Image("GoogleG").resizable().scaledToFit().frame(width: 20, height: 20)
                                    }
                                    Text(model.loginMethods?.google == true ? "Sign in with Google" : "Sign in with identity provider")
                                        .font(model.loginMethods?.google == true
                                              ? .custom("GoogleSans-Regular_Medium", size: 14)
                                              : .system(size: 14, weight: .medium))
                                    if model.loginMethods?.google == true { Spacer(minLength: 0) }
                                }.padding(.horizontal, 16)
                            }
                            .buttonStyle(SignInButtonStyle(primary: false))
                            .disabled(model.isBusy || !model.serverReady)
                        }
                        if model.loginMethods?.passwordEnabled == true {
                            HStack {
                                Button(model.localAction == .signIn ? "Accept invitation" : "Back to sign in") {
                                    model.password = ""; model.confirmPassword = ""; model.actionToken = ""
                                    model.localAction = model.localAction == .signIn ? .invitation : .signIn
                                }
                                Spacer()
                                if model.localAction == .signIn {
                                    Button("Forgot password?") { model.password = ""; model.localAction = .reset }
                                }
                            }.buttonStyle(.plain).font(.system(size: 11)).foregroundStyle(Color.accentColor).disabled(model.isBusy)
                        }
                    }
                    serverField
                    FormErrorMessage(message: model.errorMessage)
                }
            }.frame(maxWidth: 320).padding(.vertical, 28).frame(maxWidth: .infinity)
        }
        .background(Color(nsColor: .textBackgroundColor))
        .task { if model.loginMethods == nil { model.loadLoginMethods() } }
    }

    private var localFields: some View {
        VStack(alignment: .leading, spacing: 8) {
            if model.localAction != .signIn && !model.showsSetup {
                Text(model.localAction == .invitation ? "Paste the invitation from your administrator." : "Ask your administrator for a password reset credential.")
                    .font(.caption).foregroundStyle(.secondary)
                SecureField("One-time credential", text: $model.actionToken)
                    .focused($focusedField, equals: .credential).modifier(LoginFieldStyle(focused: focusedField == .credential))
            }
            if model.localAction != .reset || model.showsSetup {
                TextField("Username", text: $model.username).textContentType(.username)
                    .focused($focusedField, equals: .username).modifier(LoginFieldStyle(focused: focusedField == .username))
            }
            SecureField("Password", text: $model.password).textContentType(.password)
                .focused($focusedField, equals: .password).modifier(LoginFieldStyle(focused: focusedField == .password))
            if model.localAction != .signIn || model.showsSetup {
                SecureField("Confirm password", text: $model.confirmPassword)
                    .focused($focusedField, equals: .confirmation).modifier(LoginFieldStyle(focused: focusedField == .confirmation))
                Text("Use at least 15 characters. Usernames use 3–32 letters, digits, dots, underscores or hyphens.").font(.caption2).foregroundStyle(.secondary)
            }
        }.disabled(model.isBusy)
    }

    private var serverField: some View {
        DisclosureGroup(isExpanded: $serverExpanded) {
            HStack(spacing: 8) {
                TextField("https://clumsies.example.com", text: $model.serverOrigin)
                    .focused($focusedField, equals: .server)
                    .modifier(LoginFieldStyle(focused: focusedField == .server))
                    .accessibilityLabel("Server address")
                    .onSubmit { model.loadLoginMethods() }
                Button("Connect") { model.loadLoginMethods() }
                    .buttonStyle(SignInButtonStyle(primary: true))
                    .frame(width: 88)
                    .disabled(model.serverReady)
            }
            .padding(.top, 6)
        } label: {
            HStack(spacing: 8) {
                Text("Server address")
                Spacer(minLength: 0)
                Text(model.serverOrigin).lineLimit(1).truncationMode(.middle)
            }.font(.system(size: 10)).foregroundStyle(.secondary)
        }
        .disabled(model.isBusy)
        .padding(.top, 4)
    }

    private var setupFields: some View {
        VStack(alignment: .leading, spacing: 13) {
            if !model.setupCodeConfigured {
                setupWarning(String(localized: "Set CLUMSIES_SETUP_CODE in the Server deployment before continuing."))
            }
            if !model.oidcConfigured && !model.setupWithPassword {
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

private struct LoginFieldStyle: ViewModifier {
    let focused: Bool

    func body(content: Content) -> some View {
        content.textFieldStyle(.plain).font(.system(size: 13))
            .padding(.horizontal, 12).frame(height: 36)
            .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6)
                .strokeBorder(focused ? Color.accentColor : Color(nsColor: .separatorColor), lineWidth: 1))
    }
}

private struct SignInButtonStyle: ButtonStyle {
    let primary: Bool
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.colorScheme) private var colorScheme

    func makeBody(configuration: Configuration) -> some View {
        let dark = colorScheme == .dark
        let fill = primary ? (isEnabled ? Color.accentColor : Color.primary.opacity(0.06))
            : (dark ? Color(red: 0.075, green: 0.075, blue: 0.078) : .white)
        configuration.label.font(.system(size: 14, weight: .semibold))
            .frame(maxWidth: .infinity).frame(height: 40)
            .foregroundStyle(!isEnabled ? Color.secondary : primary ? .white : (dark ? Color(red: 0.89, green: 0.89, blue: 0.89) : Color(red: 0.12, green: 0.12, blue: 0.12)))
            .background(fill, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(
                primary ? .clear : (dark ? Color(red: 0.56, green: 0.57, blue: 0.56) : Color(red: 0.455, green: 0.467, blue: 0.459)), lineWidth: 1))
            .overlay(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(configuration.isPressed ? 0.08 : 0)))
            .contentShape(RoundedRectangle(cornerRadius: 6))
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

        }
        .frame(maxWidth: 430, alignment: .leading)
        .pageFeedback(state.errorMessage) { Task { await state.load() } }
        .automaticRefresh(id: identity) {
            guard !state.isLoading, state.mutatingID == nil else { return }
            await state.load()
        }
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
                Text(member.identityLabel)
                    .font(.caption.weight(.medium))
                    .lineLimit(1)
                Text(member.loginLabel)
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
