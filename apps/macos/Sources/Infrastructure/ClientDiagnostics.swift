import AppKit
import CryptoKit
import Foundation
import OSLog

enum ClientDiagnostics {
    @TaskLocal static var requestID: String?
    // Hosted tests never write to the installed application's logs.
    @TaskLocal static var testLog: ClientLog?
    private static let log = ClientLog(directory: ClumsiesIdentifiers.daemonLogDirectoryURL)

    static func record(_ event: String, _ fields: [String: String] = [:]) {
        if let testLog { testLog.record(event, fields) }
        else if NSClassFromString("XCTestCase") == nil { log.record(event, fields) }
    }

    static func operation<T: Sendable>(
        layer: String, method: String,
        _ action: @Sendable () async throws -> T
    ) async throws -> T {
        let id = requestID ?? "req_" + UUID().uuidString.lowercased()
        return try await $requestID.withValue(id) {
            let started = ContinuousClock.now
            do {
                return try await action()
            } catch {
                let elapsed = started.duration(to: .now).components
                var fields = failureFields(error)
                fields["request_id"] = id
                fields["layer"] = layer
                fields["method"] = identifier(method)
                fields["elapsed_ms"] = String(elapsed.seconds * 1_000 + elapsed.attoseconds / 1_000_000_000_000_000)
                record(error is CancellationError ? "request_cancelled" : "request_failed", fields)
                throw error
            }
        }
    }

    static func failureFields(_ error: Error) -> [String: String] {
        if case DaemonXPCError.daemon(let payload) = error {
            var fields = ["kind": identifier(payload.code)]
            fields["daemon_request_id"] = payload.requestId.map(identifier)
            fields["timeout"] = payload.details?.timeout.map(String.init)
            fields["connect"] = payload.details?.connect.map(String.init)
            fields["causes"] = payload.details?.causes?.map { identifier($0.kind) }.joined(separator: ",")
            return fields
        }
        if let error = error as? DaemonXPCError {
            switch error {
            case .requestTimedOut: return ["kind": "xpc_timeout"]
            case .connectionFailed: return ["kind": "xpc_connection"]
            case .invalidRequest: return ["kind": "xpc_encode"]
            case .invalidReply: return ["kind": "xpc_decode"]
            case .daemon: break
            }
        }
        if error is DecodingError { return ["kind": "decode"] }
        if error is EncodingError { return ["kind": "encode"] }
        if error is CancellationError { return ["kind": "cancelled"] }
        let nsError = error as NSError
        let domain = [NSURLErrorDomain, NSCocoaErrorDomain, NSPOSIXErrorDomain].contains(nsError.domain) ? nsError.domain : "application"
        var fields = ["kind": domain, "error_code": String(nsError.code)]
        if let underlying = nsError.userInfo[NSUnderlyingErrorKey] as? NSError {
            fields["underlying_code"] = String(underlying.code)
        }
        return fields
    }

    static func identifier(_ value: String) -> String {
        guard !value.isEmpty, value.utf8.count <= 128,
              value.utf8.allSatisfy({ (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 95 || $0 == 46 }) else { return "unknown" }
        return value
    }

    static func route(_ path: String) -> String {
        let parts = path.split(separator: "?", maxSplits: 1).first?.split(separator: "/") ?? []
        guard parts.count >= 3, parts[0] == "api", parts[1] == "v1",
              ["reviews", "drafts", "projects", "auth", "me", "health", "setup", "memories", "users", "organizations", "commits"].contains(String(parts[2])) else { return "/api/v1/:resource" }
        return "/api/v1/\(parts[2])"
    }

    // Covers native login, setup and administrator recovery, which bypass the daemon.
    static func data(for original: URLRequest, using transport: URLSession) async throws -> (Data, URLResponse) {
        try await operation(layer: "http", method: original.httpMethod ?? "GET") {
            var request = original
            request.setValue(requestID, forHTTPHeaderField: "x-clumsies-request-id")
            request.setValue(requestID, forHTTPHeaderField: "x-request-id")
            let started = ContinuousClock.now
            var fields = ["request_id": requestID ?? "", "route": route(request.url?.path ?? ""), "method": request.httpMethod ?? "GET", "body_bytes": String(request.httpBody?.count ?? 0)]
            record("http_started", fields)
            let (data, response) = try await transport.data(for: request)
            let elapsed = started.duration(to: .now).components
            fields["elapsed_ms"] = String(elapsed.seconds * 1_000 + elapsed.attoseconds / 1_000_000_000_000_000)
            if let http = response as? HTTPURLResponse {
                fields["status"] = String(http.statusCode)
                fields["server_request_id"] = http.value(forHTTPHeaderField: "x-request-id").map(identifier)
            }
            let failed = (response as? HTTPURLResponse).map { !(200..<300).contains($0.statusCode) } ?? true
            record(failed ? "http_failed" : "http_completed", fields)
            return (data, response)
        }
    }

    @MainActor static func presentExport() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "Clumsies-Diagnostics-\(Int(Date().timeIntervalSince1970))"
        panel.title = "Export Diagnostics"
        panel.prompt = "Export"
        panel.begin { result in
            guard result == .OK, let destination = panel.url else { return }
            Task { @MainActor in
                do {
                    let health = try? await DaemonXPCClient().health()
                    var metadata = Self.metadata
                    metadata["app_executable_sha256"] = (try? executableHash()) ?? "unavailable"
                    metadata["daemon_version"] = health?.daemonVersion ?? "unavailable"
                    metadata["daemon_build"] = health?.agentRuntime.buildId ?? "unavailable"
                    let daemonDirectory = health.map { URL(fileURLWithPath: $0.logDir) } ?? ClumsiesIdentifiers.daemonLogDirectoryURL
                    try export(to: destination, appDirectory: ClumsiesIdentifiers.daemonLogDirectoryURL, daemonDirectory: daemonDirectory, metadata: metadata)
                    NSWorkspace.shared.activateFileViewerSelecting([destination])
                } catch {
                    record("diagnostic_export_failed", failureFields(error))
                    NSAlert(error: error).runModal()
                }
            }
        }
    }

    static var metadata: [String: String] {
        ["collected_at": Date().ISO8601Format(), "app_version": Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown",
         "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown",
         "bundle_id": Bundle.main.bundleIdentifier ?? "unknown", "app_pid": String(ProcessInfo.processInfo.processIdentifier),
         "instance": ClumsiesIdentifiers.developmentInstanceID ?? "stable", "service": ClumsiesIdentifiers.daemon,
         "os": ProcessInfo.processInfo.operatingSystemVersionString]
    }

    private static func executableHash() throws -> String {
        guard let executable = Bundle.main.executableURL else { return "unavailable" }
        let handle = try FileHandle(forReadingFrom: executable)
        defer { try? handle.close() }
        var hash = SHA256()
        while let data = try handle.read(upToCount: 64 * 1024), !data.isEmpty { hash.update(data: data) }
        return hash.finalize().map { String(format: "%02x", $0) }.joined()
    }

    static func export(to destination: URL, appDirectory: URL, daemonDirectory: URL, metadata: [String: String]) throws {
        let fm = FileManager.default
        // A fresh destination prevents merging with an unrelated previous export.
        guard !fm.fileExists(atPath: destination.path) else { throw CocoaError(.fileWriteFileExists) }
        try fm.createDirectory(at: destination, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var report = metadata
        let families: [(String, URL, Int)] = [
            ("client.log", appDirectory, 3), ("daemon.log", daemonDirectory, 3),
            ("clumsiesd.crash.log", daemonDirectory, 3),
            ("clumsiesd.err.log", daemonDirectory, 0), ("clumsiesd.out.log", daemonDirectory, 0),
            ("app.err.log", appDirectory, 0), ("app.out.log", appDirectory, 0)
        ]
        for (base, directory, archives) in families {
            for index in 0...archives {
                let name = base + (index == 0 ? "" : ".\(index)")
                let source = directory.appending(path: name)
                let data: Data
                do {
                    let values = try source.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
                    guard values.isRegularFile == true, values.isSymbolicLink != true else { throw CocoaError(.fileReadUnsupportedScheme) }
                    let handle = try FileHandle(forReadingFrom: source)
                    defer { try? handle.close() }
                    let size = try handle.seekToEnd()
                    try handle.seek(toOffset: size > ClientLog.maxBytes ? size - ClientLog.maxBytes : 0)
                    data = try handle.read(upToCount: Int(ClientLog.maxBytes)) ?? Data()
                    report[name] = size > ClientLog.maxBytes ? "last 4 MiB" : "included"
                } catch {
                    report[name] = fm.fileExists(atPath: source.path) ? "unreadable" : "missing"
                    continue
                }
                try data.write(to: destination.appending(path: name), options: .atomic)
            }
        }
        try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
            .write(to: destination.appending(path: "manifest.json"), options: .atomic)
    }
}

// One App process owns each instance's log directory; dev instances have separate directories.
final class ClientLog: @unchecked Sendable {
    static let maxBytes: UInt64 = 4 * 1024 * 1024
    private let lock = NSLock()
    private let directory: URL
    private let limit: UInt64
    private var reportedFailure = false
    private var prepared = false
    private let fallback = Logger(subsystem: ClumsiesIdentifiers.namespace, category: "Diagnostics")

    init(directory: URL, limit: UInt64 = maxBytes) { self.directory = directory; self.limit = limit }

    func record(_ event: String, _ fields: [String: String] = [:]) {
        lock.withLock {
            do {
                var record = fields
                record["event"] = event
                record["time"] = Date().ISO8601Format()
                record["pid"] = String(ProcessInfo.processInfo.processIdentifier)
                let data = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys]) + Data([10])
                guard data.count <= limit else { return }
                let fm = FileManager.default
                try fm.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
                if !prepared {
                    for index in 0...3 {
                        let old = directory.appending(path: "client.log" + (index == 0 ? "" : ".\(index)"))
                        if let size = try? fm.attributesOfItem(atPath: old.path)[.size] as? UInt64, size > limit {
                            let handle = try FileHandle(forUpdating: old)
                            defer { try? handle.close() }
                            try handle.seek(toOffset: size - limit)
                            let tail = try handle.read(upToCount: Int(limit)) ?? Data()
                            try handle.truncate(atOffset: 0)
                            try handle.seek(toOffset: 0)
                            try handle.write(contentsOf: tail)
                        }
                    }
                    prepared = true
                }
                let path = directory.appending(path: "client.log")
                let size = (try? fm.attributesOfItem(atPath: path.path)[.size] as? UInt64) ?? 0
                if size + UInt64(data.count) > limit {
                    for index in stride(from: 3, through: 1, by: -1) {
                        let source = directory.appending(path: index == 1 ? "client.log" : "client.log.\(index - 1)")
                        let target = directory.appending(path: "client.log.\(index)")
                        if fm.fileExists(atPath: target.path) { try fm.removeItem(at: target) }
                        if fm.fileExists(atPath: source.path) { try fm.moveItem(at: source, to: target) }
                    }
                }
                if !fm.fileExists(atPath: path.path) {
                    guard fm.createFile(atPath: path.path, contents: nil, attributes: [.posixPermissions: 0o600]) else { throw CocoaError(.fileWriteUnknown) }
                }
                let handle = try FileHandle(forWritingTo: path)
                defer { try? handle.close() }
                try handle.seekToEnd()
                try handle.write(contentsOf: data)
                reportedFailure = false
            } catch {
                // A broken diagnostics disk must not break business operations; make the sink failure visible.
                if !reportedFailure { fallback.error("Client diagnostic file is unavailable; inspect log directory permissions and free space.") }
                reportedFailure = true
            }
        }
    }
}
