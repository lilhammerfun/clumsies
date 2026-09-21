import Foundation

enum AppLanguage: String, CaseIterable {
    case system
    case english = "en"
    case simplifiedChinese = "zh-Hans"

    private static let defaultsKey = "AppleLanguages"

    var title: String {
        switch self {
        case .system: String(localized: "Follow System")
        case .english: "English"
        case .simplifiedChinese: "简体中文"
        }
    }

    static func restored(
        from defaults: UserDefaults = .standard,
        domainName: String = Bundle.main.bundleIdentifier ?? ""
    ) -> Self {
        // Read only the app override; inherited system languages mean Follow System.
        guard let languages = defaults.persistentDomain(forName: domainName)?[defaultsKey] as? [String],
              !languages.isEmpty else { return .system }
        let preferred = Bundle.preferredLocalizations(
            from: [english.rawValue, simplifiedChinese.rawValue], forPreferences: languages
        ).first
        return preferred.flatMap(Self.init(rawValue:)) ?? .english
    }

    func persist(in defaults: UserDefaults = .standard) {
        if self == .system {
            defaults.removeObject(forKey: Self.defaultsKey)
        } else {
            defaults.set([rawValue], forKey: Self.defaultsKey)
        }
    }
}
