import Foundation

enum AppBundleRuntimeLocationError: UserFacingError, Sendable {
    case translocated

    var errorDescription: String? {
        switch self {
        case .translocated:
            String(localized: "Clumsies is running from a temporary macOS App Translocation path. Quit the App, move Clumsies.app to /Applications or ~/Applications, then open it again. No daemon or Agent integration was changed.")
        }
    }
}

enum AppBundleRuntimeLocation {
    static var defaultLogDirectoryURL: URL {
        ClumsiesIdentifiers.daemonLogDirectoryURL
    }

    static func requireStable(_ bundleURL: URL) throws {
        let components = bundleURL.standardizedFileURL.resolvingSymlinksInPath().pathComponents
        if components.contains("AppTranslocation") {
            throw AppBundleRuntimeLocationError.translocated
        }
    }
}
