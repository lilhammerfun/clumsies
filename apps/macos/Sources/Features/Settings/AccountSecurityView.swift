import SwiftUI

struct AccountSecurityView: View {
    @EnvironmentObject private var context: WorkspaceContext
    @Environment(\.dismiss) private var dismiss
    @State private var credentials: AccountCredentialStatus?
    @State private var methods: NativeLoginMethods?
    @State private var username = ""
    @State private var currentPassword = ""
    @State private var password = ""
    @State private var confirmation = ""
    @State private var busy = false
    @State private var error: String?
    @State private var notice: String?

    var body: some View {
        Form {
            Section("Login methods") {
                if let credentials {
                    LabeledContent("Username", value: credentials.username ?? String(localized: "Not set"))
                    LabeledContent("Identity provider", value: credentials.oidcEmail ?? String(localized: "Not bound"))
                    if credentials.passwordSet {
                        SecureField("Current password", text: $currentPassword)
                    } else {
                        Text("Sign in again before adding a password or binding an identity provider. Your sign-in must be less than five minutes old.").font(.caption).foregroundStyle(.secondary)
                    }
                    if methods?.oidcEnabled == true && credentials.oidcEmail == nil {
                        Button(methods?.google == true ? "Bind Google account" : "Bind identity provider") { bind() }
                    }
                } else { ProgressView() }
            }
            if methods?.passwordEnabled == true, let credentials {
                Section(credentials.passwordSet ? "Change password" : "Add password") {
                    if credentials.username == nil { TextField("Username", text: $username) }
                    SecureField("New password", text: $password)
                    SecureField("Confirm password", text: $confirmation)
                    Text("Use at least 15 characters.").font(.caption).foregroundStyle(.secondary)
                    Text("Changing your password signs out your other sessions.").font(.caption).foregroundStyle(.secondary)
                    Button("Save password") { save() }.disabled(password.isEmpty || password != confirmation)
                }
            }
            FormErrorMessage(message: error)
            if let notice { Text(notice).foregroundStyle(.secondary) }
            Button("Done") { dismiss() }
        }
        .formStyle(.grouped).frame(width: 480, height: 460)
        .disabled(busy).interactiveDismissDisabled(busy)
        .task { await load() }
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
        run {
            try await AccountSecurityClient(daemon: context.daemon, server: context.server).changePassword(.init(username: username.isEmpty ? nil : username, currentPassword: credentials?.passwordSet == true ? currentPassword : nil, password: password))
        }
    }

    private func bind() {
        run {
            try await AccountSecurityClient(daemon: context.daemon, server: context.server).bindOIDC(currentPassword: credentials?.passwordSet == true ? currentPassword : nil)
        }
    }

    private func run(_ operation: @escaping @MainActor () async throws -> NativeAuthenticatedSession) {
        guard !busy, !context.isSigningOut else { return }
        busy = true; error = nil; notice = nil
        let generation = context.authorityGeneration
        Task { @MainActor in
            defer { busy = false; currentPassword = ""; password = ""; confirmation = "" }
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
                notice = String(localized: "Login methods updated.")
                await load()
            } catch {
                guard context.authorityGeneration == generation else { return }
                self.error = error.actionMessage
            }
        }
    }
}
