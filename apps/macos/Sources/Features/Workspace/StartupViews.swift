import AppKit
import SwiftUI

struct LaunchView: View {
    @State private var loadingStageText: String = String(localized: "Connecting to resident daemon…")

    private let brandAccent = Color(red: 0.88, green: 0.32, blue: 0.60)

    var body: some View {
        VStack(spacing: 24) {
            Spacer()

            VStack(spacing: 16) {
                BrandLogoView(size: 68, isBreathing: true)

                VStack(spacing: 4) {
                    Text(ClumsiesIdentifiers.appDisplayName)
                        .font(.title2.weight(.semibold))
                        .foregroundStyle(.primary)

                    Text("The Collaborative Memory Platform for Agent Coding")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            }

            VStack(spacing: 10) {
                ProgressView()
                    .progressViewStyle(.linear)
                    .tint(brandAccent)
                    .frame(width: 160)
                    .controlSize(.small)

                Text(loadingStageText)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            .padding(.top, 4)

            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .textBackgroundColor))
        .onAppear {
            Task {
                try? await Task.sleep(nanoseconds: 700_000_000)
                withAnimation(.easeInOut(duration: 0.3)) {
                    loadingStageText = String(localized: "Syncing team memory…")
                }
            }
        }
    }
}

struct FailureView: View {
    let message: String
    let retry: () -> Void
    var onAdministratorRecovery: (() -> Void)?
    var onShowLogs: (() -> Void)?
    @State private var copied = false

    var body: some View {
        VStack(spacing: 18) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 38))
                .foregroundStyle(.yellow)
            Text("Clumsies could not start local daemon")
                .font(.title3.weight(.semibold))

            VStack(alignment: .leading, spacing: 8) {
                Text(message)
                    .font(.system(.callout, design: .monospaced))
                    .foregroundStyle(.primary)
                    .textSelection(.enabled)
                    .lineLimit(8)
            }
            .padding(14)
            .background(Color(nsColor: .controlBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(Color(nsColor: .separatorColor), lineWidth: 1)
            )
            .frame(maxWidth: 520)

            VStack(spacing: 12) {
                Button("Try Again", action: retry)
                    .buttonStyle(.borderedProminent)

                if let onAdministratorRecovery {
                    Button("Administrator Recovery", action: onAdministratorRecovery)
                        .buttonStyle(.bordered)
                }

                Button("Reveal Logs in Finder") {
                    if let onShowLogs {
                        onShowLogs()
                    } else {
                        let url = AppBundleRuntimeLocation.defaultLogDirectoryURL
                        if FileManager.default.fileExists(atPath: url.path) {
                            NSWorkspace.shared.activateFileViewerSelecting([url])
                        } else {
                            NSWorkspace.shared.open(url)
                        }
                    }
                }
                .buttonStyle(.bordered)

            }

            HStack(spacing: 12) {
                Button("Export Diagnostics…") { DiagnosticsExport.present() }
                    .buttonStyle(.bordered)

                Button(copied ? "Copied!" : "Copy Diagnostics") {
                    let pasteboard = NSPasteboard.general
                    pasteboard.clearContents()
                    let report = """
                    Clumsies Startup Diagnostics:
                    Error: \(message)
                    Log Directory: \(AppBundleRuntimeLocation.defaultLogDirectoryURL.path)
                    \(ClientDiagnostics.metadata.sorted { $0.key < $1.key }.map { "\($0.key): \($0.value)" }.joined(separator: "\n"))
                    """
                    pasteboard.setString(report, forType: .string)
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(2))
                        copied = false
                    }
                }
                .buttonStyle(.bordered)
            }
        }
        .padding(38)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .textBackgroundColor))
    }
}
