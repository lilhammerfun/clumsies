import AppKit
import XCTest
@testable import Clumsies

final class FileSymbolCatalogTests: XCTestCase {
    private var catalog: FileSymbolCatalog!

    override func setUpWithError() throws {
        catalog = try FileSymbolCatalog.bundled()
    }

    func testResolvesExactFileNamesBeforeExtensions() {
        XCTAssertEqual(catalog.symbolName(for: "rules/license.md"), "license")
        XCTAssertEqual(catalog.symbolName(for: "config/.gitignore"), "git")
        XCTAssertEqual(catalog.symbolName(for: "containers/Dockerfile"), "docker")
    }

    func testResolvesLongestCompoundExtension() {
        XCTAssertEqual(catalog.symbolName(for: "types/api.d.ts"), "ts-types")
        XCTAssertEqual(catalog.symbolName(for: "exports/archive.tar.gz"), "compressed")
        XCTAssertEqual(catalog.symbolName(for: "components/card.component.dart"), "angular-component")
    }

    func testResolvesExtensionsWithoutDependingOnCase() {
        XCTAssertEqual(catalog.symbolName(for: "context/README.MD"), "markdown")
        XCTAssertEqual(catalog.symbolName(for: "Sources/Memory.SWIFT"), "swift")
    }

    func testResolvesLanguageIdAliasesUsedByTheUpstreamTheme() {
        XCTAssertEqual(catalog.symbolName(for: "styles/memory.less"), "brackets-sky")
    }

    func testUnknownFilesUseTheThemeDefault() {
        XCTAssertEqual(catalog.symbolName(for: "context/unknown.clumsies-format"), "document")
        XCTAssertEqual(catalog.symbolName(for: "context/NOTICE"), "document")
    }

    func testEveryReferencedFileIconIsBundled() {
        XCTAssertEqual(catalog.validationFailures(), [])
    }

    func testBundledSVGCanBeLoadedByAppKit() {
        let image = NSImage(contentsOf: catalog.iconURL(for: "context/README.md"))
        XCTAssertNotNil(image)
    }
}
