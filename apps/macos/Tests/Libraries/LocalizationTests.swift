import Foundation
import XCTest
@testable import Clumsies

final class LocalizationTests: XCTestCase {
    private func languageBundle(_ language: String) throws -> Bundle {
        let path = try XCTUnwrap(Bundle.main.path(forResource: language, ofType: "lproj"))
        return try XCTUnwrap(Bundle(path: path))
    }

    func testPackagedLanguagesAndEnglishFallback() throws {
        XCTAssertEqual(Bundle.main.developmentLocalization, "en")
        XCTAssertTrue(Bundle.main.localizations.contains("en"))
        XCTAssertTrue(Bundle.main.localizations.contains("zh-Hans"))
        XCTAssertEqual(Bundle.preferredLocalizations(from: ["en", "zh-Hans"], forPreferences: ["fr"]), ["en"])
        let english = try languageBundle("en")
        let chinese = try languageBundle("zh-Hans")
        XCTAssertEqual(String(localized: "Inbox", bundle: english), "Inbox")
        XCTAssertEqual(String(localized: "Inbox", bundle: chinese), "收件箱")
        XCTAssertEqual(String(localized: "Settings…", bundle: chinese), "设置…")
        XCTAssertEqual(String(localized: "Bundle", bundle: chinese), "Bundle")
        XCTAssertEqual(String(localized: "No Bundle", bundle: chinese), "不使用 Bundle")
        XCTAssertEqual(String(localized: "Create Project", bundle: chinese), "创建项目")
        XCTAssertEqual(String(localized: "Maintainer", bundle: chinese), "维护者")
        XCTAssertEqual(String(localized: "Language", bundle: chinese), "语言")
        XCTAssertEqual(String(localized: "App language", bundle: chinese), "应用语言")
        XCTAssertEqual(String(localized: "Follow System", bundle: chinese), "跟随系统")
        XCTAssertEqual(String(localized: "Restart and Apply", bundle: chinese), "重启并应用")
        XCTAssertEqual(String(localized: "Open", bundle: chinese), "打开")
        XCTAssertEqual(String(localized: "Welcome to Clumsies", bundle: chinese), "欢迎使用 Clumsies")
        XCTAssertEqual(String(localized: "Access Changes", bundle: chinese), "访问权限变更")
        XCTAssertEqual(String(localized: "Read Message", bundle: chinese), "阅读消息")
        XCTAssertEqual(String(localized: "\("Mia") changed your project role: \("Member") → \("Admin").", bundle: chinese), "Mia 将你的项目角色从Member改为Admin。")
        for count in [0, 1, 2, 25] {
            XCTAssertEqual(String(localized: "\(count) requests", bundle: english, locale: Locale(identifier: "en")), "\(count) \(count == 1 ? "request" : "requests")")
            XCTAssertEqual(String(localized: "\(count) requests", bundle: chinese, locale: Locale(identifier: "zh-Hans")), "\(count) 次请求")
        }
        XCTAssertEqual(String(localized: "Delete \("notes")?", bundle: chinese), "删除 notes？")
    }

    @MainActor
    func testDisplayLabelsFollowLanguageWithoutChangingIdentifiers() {
        let isChinese = Bundle.main.preferredLocalizations.first == "zh-Hans"
        XCTAssertEqual(AppLanguage.system.title, isChinese ? "跟随系统" : "Follow System")
        XCTAssertEqual(AppLanguage.english.title, "English")
        XCTAssertEqual(AppLanguage.simplifiedChinese.title, "简体中文")
        XCTAssertEqual(WorkspaceSection.inbox.title, isChinese ? "收件箱" : "Inbox")
        XCTAssertEqual(InboxMessageType.reviewRequests.title, isChinese ? "评审请求" : "Review Requests")
        XCTAssertEqual(InboxMessageType.reviewRequests.rawValue, "Review Requests")
        XCTAssertEqual(ReviewReconciliationState.conflict.title, isChinese ? "冲突" : "Conflict")
        XCTAssertEqual(ReviewReconciliationState.conflict.rawValue, "Conflict")
        XCTAssertEqual(ReviewStatusFilter.open.title, isChinese ? "待评审" : "Open")
        XCTAssertEqual(ReviewStatusIndicator.title(for: "open"), isChinese ? "待评审" : "Open")
        XCTAssertEqual(AdminHealthStatus.down.title, isChinese ? "不可用" : "Down")
        XCTAssertEqual(AdminHealthStatus.down.rawValue, "down")
        XCTAssertEqual(SettingsDestination.search(isChinese ? "语言" : "language", canAdminister: false), [.pane(.general)])
        XCTAssertEqual(SettingsDestination.search("language", canAdminister: false), [.pane(.general)])
        for query in ["中文", "简体中文", "跟随系统", "Follow System", "updates"] {
            XCTAssertEqual(SettingsDestination.search(query, canAdminister: false), [.pane(.general)], query)
        }
    }

    func testCatalogHasReviewedTranslationsForEveryLocalizableKey() throws {
        let appRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let data = try Data(contentsOf: appRoot.appending(path: "Resources/Localizable.xcstrings"))
        let catalog = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let strings = try XCTUnwrap(catalog["strings"] as? [String: [String: Any]])
        XCTAssertFalse(strings.isEmpty)
        for (key, entry) in strings where entry["shouldTranslate"] as? Bool != false {
            let languages = try XCTUnwrap(entry["localizations"] as? [String: Any], key)
            for language in ["en", "zh-Hans"] {
                let localization = try XCTUnwrap(languages[language], "\(key): \(language)")
                verifyUnits(localization, key: key, language: language)
            }
        }
    }

    private func verifyUnits(_ value: Any, key: String, language: String) {
        guard let object = value as? [String: Any] else { return }
        if let unit = object["stringUnit"] as? [String: String] {
            XCTAssertEqual(unit["state"], "translated", "\(key): \(language)")
            XCTAssertFalse(unit["value"]?.isEmpty ?? true, "\(key): \(language)")
        }
        for child in object.values { verifyUnits(child, key: key, language: language) }
    }
}
