import Foundation

struct DaemonBootstrapState: Equatable, Sendable {
    let installed: Bool
    let running: Bool
    let pid: Int?
    let error: String?
}

enum DaemonBootstrapError: LocalizedError, Sendable {
    case daemonBinaryMissing
    case daemonCommand(String)
    case invalidDevelopmentConfiguration([String])
    case invalidStatus

    var errorDescription: String? {
        switch self {
        case .daemonBinaryMissing:
            return "The Clumsies daemon is missing from the application bundle."
        case .daemonCommand(let message):
            return message
        case .invalidDevelopmentConfiguration(let missing):
            return "The Clumsies Dev App is missing required instance settings: \(missing.joined(separator: ", "))."
        case .invalidStatus:
            return "The Clumsies daemon returned an invalid LaunchAgent status."
        }
    }
}

struct DaemonBootstrapController: Sendable {
    static let label = ClumsiesIdentifiers.daemon

    private struct BootstrapStatus: Decodable {
        let installed: Bool
        let runtime: RuntimeStatus
    }

    private struct RuntimeStatus: Decodable {
        let running: Bool
        let pid: UInt32?
        let lastError: String?
    }

    private struct ProcessResult {
        let status: Int32
        let output: String
    }

    func ensureRunning() async throws -> DaemonBootstrapState {
        try await Task.detached(priority: .userInitiated) {
            try runStatusCommand("--reconcile-launch-agent")
        }.value
    }

    func status() async -> DaemonBootstrapState {
        do {
            return try await Task.detached {
                try runStatusCommand("--status-launch-agent")
            }.value
        } catch {
            return .init(
                installed: launchAgentPlistExists(),
                running: false,
                pid: nil,
                error: error.localizedDescription
            )
        }
    }

    static func decodeState(_ data: Data) throws -> DaemonBootstrapState {
        let status: BootstrapStatus
        do {
            status = try JSONCoding.decoder().decode(BootstrapStatus.self, from: data)
        } catch {
            throw DaemonBootstrapError.invalidStatus
        }
        return .init(
            installed: status.installed,
            running: status.runtime.running,
            pid: status.runtime.pid.map(Int.init),
            error: status.runtime.lastError
        )
    }

    private func runStatusCommand(_ command: String) throws -> DaemonBootstrapState {
        let result = try runDaemon([command])
        guard result.status == 0 else {
            throw DaemonBootstrapError.daemonCommand(
                result.output.isEmpty
                    ? "clumsiesd exited with status \(result.status)."
                    : result.output
            )
        }
        return try Self.decodeState(Data(result.output.utf8))
    }

    private func runDaemon(_ arguments: [String]) throws -> ProcessResult {
        let missingSettings = ClumsiesIdentifiers.missingDevelopmentSettings
        guard missingSettings.isEmpty else {
            throw DaemonBootstrapError.invalidDevelopmentConfiguration(missingSettings)
        }
        let process = Process()
        let pipe = Pipe()
        process.executableURL = try daemonBinaryURL()
        process.arguments = arguments
        process.environment = ClumsiesIdentifiers.daemonEnvironment()
        process.standardOutput = pipe
        process.standardError = pipe
        try process.run()
        process.waitUntilExit()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        return .init(
            status: process.terminationStatus,
            output: String(decoding: data, as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
        )
    }

    private func daemonBinaryURL() throws -> URL {
        // Reject App Translocation before honoring any development executable
        // override: no code path may reconcile or persist a daemon location
        // while this App bundle itself has an ephemeral randomized path.
        try AppBundleRuntimeLocation.requireStable(Bundle.main.bundleURL)
        if let override = ProcessInfo.processInfo.environment["CLUMSIES_DAEMON_PROGRAM"],
           !override.isEmpty {
            return URL(fileURLWithPath: override)
        }
        guard let url = Bundle.main.resourceURL?.appending(path: "clumsiesd"),
              FileManager.default.isExecutableFile(atPath: url.path) else {
            throw DaemonBootstrapError.daemonBinaryMissing
        }
        return url
    }

    private func launchAgentPlistExists() -> Bool {
        let url = ClumsiesIdentifiers.daemonLaunchAgentPlistURL
        return FileManager.default.fileExists(atPath: url.path)
    }
}
