import AppKit
import CryptoKit
import Foundation

enum DiagnosticsExport {
    @MainActor static func present() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "Clumsies-Diagnostics-\(Int(Date().timeIntervalSince1970))"
        panel.title = "Export Diagnostics"
        panel.prompt = "Export"
        panel.begin { result in
            guard result == .OK, let destination = panel.url else { return }
            Task { @MainActor in
                do {
                    let health = try? await DaemonXPCClient().health()
                    var metadata = ClientDiagnostics.metadata
                    metadata["app_executable_sha256"] = (try? executableHash()) ?? "unavailable"
                    metadata["daemon_version"] = health?.daemonVersion ?? "unavailable"
                    metadata["daemon_build"] = health?.agentRuntime.buildId ?? "unavailable"
                    let daemonDirectory = health.map { URL(fileURLWithPath: $0.logDir) } ?? ClumsiesIdentifiers.daemonLogDirectoryURL
                    try ClientDiagnostics.export(to: destination, appDirectory: ClumsiesIdentifiers.daemonLogDirectoryURL, daemonDirectory: daemonDirectory, metadata: metadata)
                    NSWorkspace.shared.activateFileViewerSelecting([destination])
                } catch {
                    ClientDiagnostics.record("diagnostic_export_failed", ClientDiagnostics.failureFields(error))
                    NSAlert(error: error).runModal()
                }
            }
        }
    }

    private static func executableHash() throws -> String {
        guard let executable = Bundle.main.executableURL else { return "unavailable" }
        let handle = try FileHandle(forReadingFrom: executable)
        defer { try? handle.close() }
        var hash = SHA256()
        while let data = try handle.read(upToCount: 64 * 1024), !data.isEmpty { hash.update(data: data) }
        return hash.finalize().map { String(format: "%02x", $0) }.joined()
    }

}
