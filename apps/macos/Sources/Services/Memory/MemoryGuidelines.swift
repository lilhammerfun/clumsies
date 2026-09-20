import Foundation

/// The resource a user is about to open, select, or create as Memory Guidelines.
struct MemoryGuidelinesSetup: Equatable, Sendable {
    enum Action: Equatable, Sendable {
        case open(String)
        case useOrganization(MemoryResource)
        case createDefault
    }

    let projectId: String
    let path: String
    let action: Action
    var organizationCommitId: String?
    var occupiedPaths: Set<String> = []

    func hasSameDestination(as other: Self) -> Bool {
        guard projectId == other.projectId, path == other.path else { return false }
        switch (action, other.action) {
        case (.open(let left), .open(let right)): return left == right
        case (.useOrganization(let left), .useOrganization(let right)): return left.id == right.id
        case (.createDefault, .createDefault): return true
        default: return false
        }
    }
}

enum MemoryGuidelines {
    static let defaultPath = "CLUMSIES.md"
    static let starterFolders = ["knowledge", "procedures", "lessons"]

    static func configuredPath(_ value: String?) -> String {
        let path = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return path.isEmpty ? defaultPath : path
    }

    static func defaultDocument() throws -> EditableMemoryDocument {
        guard let url = Bundle.main.url(forResource: "CLUMSIES", withExtension: "md") else {
            throw MemoryValidationError.invalidPath(String(localized: "The bundled memory guidelines are unavailable."))
        }
        return .init(
            title: String(localized: "Memory Guidelines"),
            path: defaultPath,
            body: try String(contentsOf: url, encoding: .utf8)
        )
    }

    /// Seed only unused folders; the user's existing layout always wins.
    static func defaultDocuments(occupiedPaths: Set<String> = []) throws -> [EditableMemoryDocument] {
        guard !occupiedPaths.contains(defaultPath) else {
            throw MemoryValidationError.invalidPath(String(localized: "CLUMSIES.md already exists. Open the existing guidelines."))
        }
        var documents = [try defaultDocument()]
        for folder in starterFolders where !occupiedPaths.contains(where: {
            $0 == folder || $0.hasPrefix(folder + "/")
        }) {
            guard let url = Bundle.main.url(
                forResource: "README", withExtension: "md", subdirectory: "MemoryStarter/\(folder)"
            ) else {
                throw MemoryValidationError.invalidPath(String(localized: "The bundled \(folder) starter is unavailable."))
            }
            documents.append(.init(
                title: folder.capitalized, path: "\(folder)/README.md",
                body: try String(contentsOf: url, encoding: .utf8)
            ))
        }
        return documents
    }

    /// Resolve the effective project document before considering shared authority.
    /// A missing custom path or pending removal must never seed a replacement.
    static func setup(
        projectId: String,
        path: String,
        items: [MemoryListItem],
        organizationResources: [MemoryResource]
    ) throws -> MemoryGuidelinesSetup {
        if let item = items.first(where: { $0.document.path == path }) {
            guard item.draft?.isDeletion != true else {
                throw MemoryValidationError.invalidPath(
                    String(localized: "A draft removes \(path). Resolve that draft before setting up memory guidelines.")
                )
            }
            return .init(projectId: projectId, path: path, action: .open(item.id))
        }
        if let resource = organizationResources.first(where: { $0.document.path == path }) {
            guard !items.contains(where: { $0.draft?.targetId == resource.id }) else {
                throw MemoryValidationError.invalidPath(
                    String(localized: "A draft changes the location of \(path). Resolve that draft or update the guidelines path.")
                )
            }
            return .init(projectId: projectId, path: path, action: .useOrganization(resource))
        }
        guard path == defaultPath else {
            throw MemoryValidationError.invalidPath(
                String(localized: "Your configured memory guidelines, \(path), could not be found. Restore that memory or correct the configured path.")
            )
        }
        return .init(projectId: projectId, path: path, action: .createDefault)
    }
}
