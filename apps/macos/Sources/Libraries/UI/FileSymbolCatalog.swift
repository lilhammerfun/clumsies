import AppKit
import Foundation
import SwiftUI

struct FileSymbolCatalog: Sendable {
    private struct Theme: Decodable {
        struct IconDefinition: Decodable {
            let iconPath: String
        }

        let file: String
        let iconDefinitions: [String: IconDefinition]
        let fileNames: [String: String]
        let fileExtensions: [String: String]
        let languageIds: [String: String]
    }

    enum CatalogError: Error {
        case missingBundleResources
        case missingDefaultIcon(String)
    }

    private let resourceRootURL: URL
    private let defaultSymbol: String
    private let iconPathsBySymbol: [String: String]
    private let fileNames: [String: String]
    private let foldedFileNames: [String: String]
    private let foldedExtensions: [String: String]
    private let extensionsBySpecificity: [String]

    init(themeData: Data, resourceRootURL: URL) throws {
        let theme = try JSONDecoder().decode(Theme.self, from: themeData)
        let iconPathsBySymbol = theme.iconDefinitions.compactMapValues { definition in
            Self.fileIconPath(from: definition.iconPath)
        }

        guard iconPathsBySymbol[theme.file] != nil else {
            throw CatalogError.missingDefaultIcon(theme.file)
        }

        let fileNames = theme.fileNames.mapValues { symbol in
            Self.resolvedSymbol(
                symbol,
                iconPathsBySymbol: iconPathsBySymbol,
                languageIds: theme.languageIds
            )
        }
        let fileExtensions = theme.fileExtensions.mapValues { symbol in
            Self.resolvedSymbol(
                symbol,
                iconPathsBySymbol: iconPathsBySymbol,
                languageIds: theme.languageIds
            )
        }
        let foldedExtensions = Self.unambiguousFoldedMap(fileExtensions)

        self.resourceRootURL = resourceRootURL
        self.defaultSymbol = theme.file
        self.iconPathsBySymbol = iconPathsBySymbol
        self.fileNames = fileNames
        self.foldedFileNames = Self.unambiguousFoldedMap(fileNames)
        self.foldedExtensions = foldedExtensions
        self.extensionsBySpecificity = foldedExtensions.keys.sorted { left, right in
            if left.count == right.count {
                return left < right
            }
            return left.count > right.count
        }
    }

    static func bundled(in bundle: Bundle = .main) throws -> FileSymbolCatalog {
        guard let resourceURL = bundle.resourceURL else {
            throw CatalogError.missingBundleResources
        }

        let rootURL = resourceURL.appendingPathComponent("FileSymbols", isDirectory: true)
        let themeURL = rootURL.appendingPathComponent("symbol-icon-theme.json")
        return try FileSymbolCatalog(
            themeData: Data(contentsOf: themeURL),
            resourceRootURL: rootURL
        )
    }

    func symbolName(for path: String) -> String {
        let fileName = path.split(separator: "/", omittingEmptySubsequences: true)
            .last
            .map(String.init) ?? path

        if let symbol = fileNames[fileName] {
            return symbol
        }

        let foldedFileName = fileName.lowercased()
        if let symbol = foldedFileNames[foldedFileName] {
            return symbol
        }

        for pathExtension in extensionsBySpecificity
            where foldedFileName.hasSuffix(".\(pathExtension)")
        {
            return foldedExtensions[pathExtension] ?? defaultSymbol
        }

        return defaultSymbol
    }

    func iconURL(for path: String) -> URL {
        let symbol = symbolName(for: path)
        let relativePath = iconPathsBySymbol[symbol]
            ?? iconPathsBySymbol[defaultSymbol]
            ?? "icons/files/document.svg"
        return resourceRootURL.appendingPathComponent(relativePath)
    }

    func validationFailures(fileManager: FileManager = .default) -> [String] {
        let referencedSymbols = Set(fileNames.values)
            .union(foldedExtensions.values)
            .union([defaultSymbol])

        return referencedSymbols.compactMap { symbol in
            guard let path = iconPathsBySymbol[symbol] else {
                return "Missing file icon definition: \(symbol)"
            }

            let url = resourceRootURL.appendingPathComponent(path)
            guard fileManager.fileExists(atPath: url.path) else {
                return "Missing file icon asset: \(path)"
            }
            return nil
        }.sorted()
    }

    private static func fileIconPath(from rawPath: String) -> String? {
        let path = rawPath.hasPrefix("./") ? String(rawPath.dropFirst(2)) : rawPath
        let components = path.split(separator: "/", omittingEmptySubsequences: false)
        guard components.count == 3,
              components[0] == "icons",
              components[1] == "files",
              !components.contains("..")
        else {
            return nil
        }
        return path
    }

    private static func resolvedSymbol(
        _ symbol: String,
        iconPathsBySymbol: [String: String],
        languageIds: [String: String]
    ) -> String {
        if iconPathsBySymbol[symbol] != nil {
            return symbol
        }
        if let languageSymbol = languageIds[symbol], iconPathsBySymbol[languageSymbol] != nil {
            return languageSymbol
        }
        return symbol
    }

    private static func unambiguousFoldedMap(_ source: [String: String]) -> [String: String] {
        var valuesByKey: [String: Set<String>] = [:]
        for (key, value) in source {
            valuesByKey[key.lowercased(), default: []].insert(value)
        }

        return valuesByKey.compactMapValues { values in
            values.count == 1 ? values.first : nil
        }
    }
}

@MainActor
private final class FileSymbolImageStore {
    static let shared = FileSymbolImageStore()

    private let catalog = try? FileSymbolCatalog.bundled()
    private let cache = NSCache<NSString, NSImage>()

    func image(for path: String) -> NSImage? {
        guard let url = catalog?.iconURL(for: path) else { return nil }
        let cacheKey = url.path as NSString
        if let image = cache.object(forKey: cacheKey) {
            return image
        }

        guard let image = NSImage(contentsOf: url) else { return nil }
        cache.setObject(image, forKey: cacheKey)
        return image
    }
}

struct FileSymbolView: View {
    let path: String

    var body: some View {
        Group {
            if let image = FileSymbolImageStore.shared.image(for: path) {
                Image(nsImage: image)
                    .resizable()
                    .interpolation(.high)
                    .scaledToFit()
            } else {
                Image(systemName: "doc")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(width: 14, height: 14)
        .accessibilityHidden(true)
    }
}
