import SwiftUI

struct AccountSecurityView: View {
    @EnvironmentObject private var context: WorkspaceContext
    @ObservedObject var navigation: SettingsNavigation
    @State private var credentials: AccountCredentialStatus?
    @State private var methods: NativeLoginMethods?
    @State private var username = ""
    @State private var currentPassword = ""
    @State private var password = ""
    @State private var confirmation = ""
    @State private var busy = false
    @State private var error: String?
    @State private var notice: String?
    private enum Action { case password, connect }
    @State private var action: Action?

    private var hasEdits: Bool {
        !username.isEmpty || !currentPassword.isEmpty || !password.isEmpty || !confirmation.isEmpty || busy
    }

    private var providerTitle: String {
        methods?.google == true ? String(localized: "Google account") : String(localized: "Single sign-on")
    }

    var body: some View {
        Form {
            if let credentials {
                Section {
                    LabeledContent("Username", value: credentials.username ?? String(localized: "Not set"))
                    if methods?.passwordEnabled == true {
                        LabeledContent("Password") {
                            Button(credentials.passwordSet ? "Change password…" : "Set password…") {
                                begin(.password)
                            }.disabled(action != nil)
                        }
                    }
                    if methods?.oidcEnabled == true || credentials.oidcEmail != nil {
                        LabeledContent(providerTitle) {
                            if let email = credentials.oidcEmail {
                                Text(email).textSelection(.enabled)
                            } else {
                                Text("Not connected").foregroundStyle(.secondary)
                                Button("Connect account") { begin(.connect) }.disabled(action != nil)
                            }
                        }
                    }
                }
                if let action {
                    Section {
                        if credentials.passwordSet {
                            SecureField("Current password", text: $currentPassword)
                        }
                        if action == .password {
                            if credentials.username == nil { TextField("Username", text: $username) }
                            SecureField("New password", text: $password)
                            SecureField("Confirm new password", text: $confirmation)
                        }
                        HStack {
                            Button("Cancel") { clearForm() }
                            Spacer()
                            if action == .password {
                                Button(credentials.passwordSet ? "Change password" : "Set password") { save() }
                                    .disabled(password.count < 15 || password != confirmation
                                              || (credentials.passwordSet && currentPassword.isEmpty)
                                              || (credentials.username == nil && username.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty))
                            } else {
                                Button("Continue") { bind() }
                                    .disabled(credentials.passwordSet && currentPassword.isEmpty)
                            }
                        }
                    } header: {
                        Text(action == .password
                             ? (credentials.passwordSet ? String(localized: "Change password") : String(localized: "Set password"))
                             : String(localized: "Verify your identity"))
                    } footer: {
                        if action == .password {
                            Text("At least 15 characters. Other sessions will be signed out.")
                        } else {
                            Text("Continue in your browser to connect your account.")
                        }
                    }
                }
            } else {
                ProgressView()
            }
            FormErrorMessage(message: error)
            if let notice { Text(notice).foregroundStyle(.secondary) }
        }
        .formStyle(.grouped)
        .disabled(busy)
        .onChange(of: hasEdits) { _, value in navigation.hasUnsavedChanges = value }
        .onDisappear { navigation.hasUnsavedChanges = false }
        .task { await load() }
    }

    private func begin(_ next: Action) {
        clearForm()
        error = nil
        notice = nil
        action = next
    }

    private func clearForm() {
        action = nil
        username = ""
        currentPassword = ""
        password = ""
        confirmation = ""
    }

    private func load() async {
        let generation = context.authorityGeneration
        do {
            let loaded: AccountCredentialStatus = try await context.server.get("/api/v1/auth/credentials")
            let supported: NativeLoginMethods = try await context.server.get("/api/v1/auth/methods")
            guard !Task.isCancelled, context.authorityGeneration == generation else { return }
            credentials = loaded; methods = supported
        } catch {
            guard !Task.isCancelled, context.authorityGeneration == generation else { return }
            self.error = error.actionMessage
        }
    }

    private func save() {
        run(success: credentials?.passwordSet == true ? String(localized: "Password changed.") : String(localized: "Password set.")) {
            try await AccountSecurityClient(daemon: context.daemon, server: context.server).changePassword(.init(username: username.isEmpty ? nil : username, currentPassword: credentials?.passwordSet == true ? currentPassword : nil, password: password))
        }
    }

    private func bind() {
        run(success: methods?.google == true ? String(localized: "Google account connected.") : String(localized: "Account connected.")) {
            try await AccountSecurityClient(daemon: context.daemon, server: context.server).bindOIDC(currentPassword: credentials?.passwordSet == true ? currentPassword : nil)
        }
    }

    private func run(success: String, _ operation: @escaping @MainActor () async throws -> NativeAuthenticatedSession) {
        guard !busy, !context.isSigningOut else { return }
        busy = true; error = nil; notice = nil
        navigation.isSaving = true
        navigation.hasUnsavedChanges = true
        let generation = context.authorityGeneration
        Task { @MainActor in
            defer {
                busy = false; currentPassword = ""; password = ""; confirmation = ""
                if context.authorityGeneration == generation { navigation.isSaving = false }
            }
            do {
                let session = try await operation()
                try await context.projectSelectionSideEffectGate.run {
                    guard context.authorityGeneration == generation, !context.isSigningOut,
                          context.account?.userId == session.currentUser.user.userId else {
                        throw CancellationError()
                    }
                    _ = try await session.install(on: context.daemon, projectId: context.activeProjectId)
                }
                guard context.authorityGeneration == generation, !context.isSigningOut else { return }
                context.account = session.currentUser.user
                clearForm()
                notice = success
                await load()
            } catch {
                guard context.authorityGeneration == generation else { return }
                self.error = error.actionMessage
            }
        }
    }
}
