import Foundation

enum ClumsiesIdentifiers {
    static let namespace = "ai.clumsies"
    static let stableAppBundleIdentifier = "ai.clumsies.desktop"
    static let stableAppDisplayName = "Clumsies"
    static let stableDaemon = "\(namespace).daemon"
    static let stableServerURL = URL(string: "https://app.clumsies.ai")!

    static let developmentInstanceID = bundleSetting("CLUMSIES_DEV_INSTANCE_ID")
    static let appDisplayName = bundleSetting("CLUMSIES_APP_DISPLAY_NAME")
        ?? stableAppDisplayName

    private static let configuredServerOrigin: ServerOrigin = {
        let value = bundleSetting("CLUMSIES_SERVER_URL") ?? stableServerURL.absoluteString
        guard let origin = try? ServerOrigin(validating: value) else {
            preconditionFailure(
                "CLUMSIES_SERVER_URL must be an HTTPS origin or a loopback HTTP origin."
            )
        }
        return origin
    }()

    static var serverURL: URL {
        ServerOriginStore(fallback: configuredServerOrigin).load().url
    }

    @discardableResult
    static func persistServerOrigin(_ input: String) throws -> URL {
        let origin = try ServerOrigin(validating: input)
        ServerOriginStore(fallback: configuredServerOrigin).save(origin)
        return origin.url
    }

    static let developmentConfigurationDetected = detectsDevelopmentConfiguration(
        bundleIdentifier: Bundle.main.bundleIdentifier,
        appDisplayName: appDisplayName,
        developmentInstanceID: developmentInstanceID,
        serverURL: configuredServerOrigin.url,
        devOnlySettingValues: [
            "CLUMSIES_DAEMON_ROOT",
            "CLUMSIES_DAEMON_CACHE_DIR",
            "CLUMSIES_DAEMON_LOG_DIR",
            "CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR",
            "CLUMSIES_CODEX_HOME",
        ].compactMap(bundleSetting)
    )
    static let daemon = daemonServiceName(
        for: developmentInstanceID,
        developmentConfigurationDetected: developmentConfigurationDetected
    )
    static let xpcReplyQueue = "\(daemon).xpc-replies"

    static var daemonLogDirectoryURL: URL {
        if let path = bundleSetting("CLUMSIES_DAEMON_LOG_DIR") {
            return URL(fileURLWithPath: path, isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appending(components: "Library", "Logs", namespace)
    }

    static var daemonLaunchAgentPlistURL: URL {
        let directory: URL
        if let path = bundleSetting("CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR") {
            directory = URL(fileURLWithPath: path, isDirectory: true)
        } else {
            directory = FileManager.default.homeDirectoryForCurrentUser
                .appending(components: "Library", "LaunchAgents")
        }
        return directory.appending(path: "\(daemon).plist", directoryHint: .notDirectory)
    }

    static var missingDevelopmentSettings: [String] {
        guard developmentConfigurationDetected else { return [] }
        var missing = [
            "CLUMSIES_DAEMON_ROOT",
            "CLUMSIES_DAEMON_CACHE_DIR",
            "CLUMSIES_DAEMON_LOG_DIR",
            "CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR",
            "CLUMSIES_CODEX_HOME",
        ].filter { bundleSetting($0) == nil }
        if developmentInstanceID == nil {
            missing.insert("CLUMSIES_DEV_INSTANCE_ID", at: 0)
        }
        if bundleSetting("CLUMSIES_SERVER_URL") == nil
            || isStableServerURL(configuredServerOrigin.url) {
            missing.append("CLUMSIES_SERVER_URL")
        }
        return missing
    }

    static func daemonEnvironment(
        base: [String: String] = ProcessInfo.processInfo.environment
    ) -> [String: String] {
        var environment = base
        environment["CLUMSIES_SERVER_URL"] = serverURL.absoluteString
        for key in [
            "CLUMSIES_DEV_INSTANCE_ID",
            "CLUMSIES_DAEMON_ROOT",
            "CLUMSIES_DAEMON_CACHE_DIR",
            "CLUMSIES_DAEMON_LOG_DIR",
            "CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR",
        ] {
            if let value = bundleSetting(key) {
                environment[key] = value
            }
        }
        if let codexHome = bundleSetting("CLUMSIES_CODEX_HOME") {
            environment["CODEX_HOME"] = codexHome
        }
        return environment
    }

    static func daemonServiceName(
        for developmentInstanceID: String?,
        developmentConfigurationDetected: Bool = false
    ) -> String {
        guard let developmentInstanceID,
              !developmentInstanceID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return developmentConfigurationDetected
                ? "\(stableDaemon).dev.invalid-configuration"
                : stableDaemon
        }
        return "\(stableDaemon).dev.\(developmentInstanceID)"
    }

    static func isStableServerURL(_ url: URL) -> Bool {
        guard let host = url.host, let stableHost = stableServerURL.host else { return false }
        let dots = CharacterSet(charactersIn: ".")
        return host.lowercased().trimmingCharacters(in: dots)
            == stableHost.lowercased().trimmingCharacters(in: dots)
    }

    static func detectsDevelopmentConfiguration(
        bundleIdentifier: String?,
        appDisplayName: String?,
        developmentInstanceID: String?,
        serverURL: URL?,
        devOnlySettingValues: [String]
    ) -> Bool {
        if let bundleIdentifier,
           bundleIdentifier.trimmingCharacters(in: .whitespacesAndNewlines)
            != stableAppBundleIdentifier {
            return true
        }
        if let appDisplayName,
           appDisplayName.trimmingCharacters(in: .whitespacesAndNewlines)
            != stableAppDisplayName {
            return true
        }
        if let developmentInstanceID,
           !developmentInstanceID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return true
        }
        if let serverURL, !isStableServerURL(serverURL) {
            return true
        }
        return devOnlySettingValues.contains {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
    }

    private static func bundleSetting(_ key: String) -> String? {
        guard let value = Bundle.main.object(forInfoDictionaryKey: key) as? String else {
            return nil
        }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
