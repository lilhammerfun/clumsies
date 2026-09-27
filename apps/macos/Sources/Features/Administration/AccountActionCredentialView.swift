import AppKit
import SwiftUI

struct AccountActionCredentialView: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var administration: AdministrationModel
    let credential: AccountActionCredential
    @State private var error: String?
    @State private var busy = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("One-time credential").font(.headline)
            Text("Send this credential privately to the member. It is shown only once.")
            Text(credential.token).font(.system(.body, design: .monospaced)).textSelection(.enabled)
            Text("Expires: \(credential.expiresAt)").font(.caption).foregroundStyle(.secondary)
            FormErrorMessage(message: error)
            HStack {
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(credential.token, forType: .string)
                }
                Button("Revoke", role: .destructive) {
                    busy = true
                    Task {
                        defer { busy = false }
                        do { try await administration.revokeAccountAction(credential); dismiss() }
                        catch { self.error = error.actionMessage }
                    }
                }
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            }
        }.padding(24).frame(width: 430).disabled(busy).interactiveDismissDisabled(busy)
    }
}
