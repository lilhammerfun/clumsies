import Foundation
import XCTest
@testable import Clumsies

final class AppLanguageTests: XCTestCase {
    func testAppOverridePersistsAndFollowSystemRemovesOnlyTheOverride() throws {
        let suite = "AppLanguageTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.register(defaults: ["AppleLanguages": ["en"]])
        defaults.set("keep", forKey: "unrelatedPreference")
        let globalLanguages = defaults.persistentDomain(forName: UserDefaults.globalDomain)?["AppleLanguages"] as? [String]

        XCTAssertEqual(AppLanguage.restored(from: defaults, domainName: suite), .system)
        for language in [AppLanguage.simplifiedChinese, .english] {
            language.persist(in: defaults)
            let reopened = try XCTUnwrap(UserDefaults(suiteName: suite))
            XCTAssertEqual(AppLanguage.restored(from: reopened, domainName: suite), language)
            XCTAssertEqual(reopened.persistentDomain(forName: suite)?["AppleLanguages"] as? [String], [language.rawValue])
        }

        AppLanguage.system.persist(in: defaults)
        XCTAssertNil(defaults.persistentDomain(forName: suite)?["AppleLanguages"])
        XCTAssertEqual(AppLanguage.restored(from: defaults, domainName: suite), .system)
        XCTAssertEqual(defaults.string(forKey: "unrelatedPreference"), "keep")
        XCTAssertEqual(defaults.persistentDomain(forName: UserDefaults.globalDomain)?["AppleLanguages"] as? [String], globalLanguages)
    }

    func testRestoresMacOSAppLanguageOverridesUsingNativeLanguageMatching() throws {
        let suite = "AppLanguageTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        for (languages, expected) in [
            (["zh-Hans-CN"], AppLanguage.simplifiedChinese),
            (["en-GB"], .english),
            (["fr", "zh-Hans"], .simplifiedChinese),
            (["fr"], .english),
            ([], .system),
        ] {
            defaults.set(languages, forKey: "AppleLanguages")
            XCTAssertEqual(AppLanguage.restored(from: defaults, domainName: suite), expected, "\(languages)")
        }
        defaults.set("invalid", forKey: "AppleLanguages")
        XCTAssertEqual(AppLanguage.restored(from: defaults, domainName: suite), .system)
    }
}
