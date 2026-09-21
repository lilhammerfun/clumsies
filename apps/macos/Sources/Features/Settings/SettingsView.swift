import AppKit
import SwiftUI

struct GeneralSettingsView: View {
    @ObservedObject var softwareUpdateController: SoftwareUpdateController
    let onRestart: () -> Void
    @State private var language = AppLanguage.restored()
    @State private var showsLanguageChangeNotice = false

    private var version: String {
        let short = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? String(localized: "Unknown")
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        return build.isEmpty ? short : "\(short) (\(build))"
    }

    var body: some View {
        Form {
            Section {
                VStack(spacing: 10) {
                    SettingsIcon(symbol: "gearshape.fill", color: .gray, size: 52)
                    Text("General").font(.system(size: 22, weight: .semibold))
                    Text("App language, information and software updates.")
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 14)
                LabeledContent("Version", value: self.version)
                    .textSelection(.enabled)
            }
            Section {
                Picker("App language", selection: $language) {
                    ForEach(AppLanguage.allCases, id: \.self) { language in
                        Text(language.title).tag(language)
                    }
                }
                .accessibilityIdentifier("app-language-picker")
                .onChange(of: language) { _, language in
                    language.persist()
                    showsLanguageChangeNotice = true
                }
            } header: {
                Text("Language")
            } footer: {
                Text("Choose a language for Clumsies without changing your Mac's language. Restart Clumsies to apply the change.")
            }
            Section("Updates") {
                Toggle(
                    "Automatically check for updates",
                    isOn: Binding(
                        get: { self.softwareUpdateController.automaticallyChecksForUpdates },
                        set: { self.softwareUpdateController.automaticallyChecksForUpdates = $0 }
                    )
                )
                Toggle(
                    "Automatically download updates",
                    isOn: Binding(
                        get: { self.softwareUpdateController.automaticallyDownloadsUpdates },
                        set: { self.softwareUpdateController.automaticallyDownloadsUpdates = $0 }
                    )
                )
                .disabled(!self.softwareUpdateController.allowsAutomaticUpdates)
                LabeledContent("Software updates") {
                    Button("Check for Updates…") { self.softwareUpdateController.checkForUpdates() }
                        .disabled(!self.softwareUpdateController.canCheckForUpdates)
                }
            }
        }
        .formStyle(.grouped)
        .font(.system(size: 13))
        .toggleStyle(.switch)
        .alert("Restart Clumsies to apply the language?", isPresented: $showsLanguageChangeNotice) {
            Button("Restart and Apply", action: onRestart)
            Button("Later", role: .cancel) {}
        } message: {
            Text("Clumsies will save pending changes and reopen automatically in the selected language.")
        }
    }
}

struct SupportSettingsView: View {
    let onShowLogs: () -> Void

    var body: some View {
        Form {
            Section {
                LabeledContent("Logs") {
                    Button("Show in Finder", action: self.onShowLogs)
                }
                LabeledContent("Diagnostics") {
                    Button("Export…") { DiagnosticsExport.present() }
                }
            } footer: {
                Text("Use logs to help investigate a problem with Clumsies.")
            }
        }
        .formStyle(.grouped)
        .font(.system(size: 13))
    }
}
