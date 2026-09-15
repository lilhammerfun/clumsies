import Foundation

enum MemoryExportError: LocalizedError {
    case empty
    case invalidPath(String)
    case conflictingPath(String)
    case contentUnavailable(String)
    case compressionFailed(Int32)

    var errorDescription: String? {
        switch self {
        case .empty:
            "There are no memory files to export."
        case .invalidPath(let path):
            "The memory path cannot be exported safely: \(path)"
        case .conflictingPath(let path):
            "Multiple memories use conflicting file paths: \(path)"
        case .contentUnavailable(let path):
            "The full content of \(path) is unavailable. Refresh or reconcile it before exporting."
        case .compressionFailed(let status):
            "Could not create the memory ZIP archive (exit status \(status))."
        }
    }
}

enum MemoryArchive {
    static func write(_ documents: [EditableMemoryDocument], to destination: URL) throws {
        guard !documents.isEmpty else { throw MemoryExportError.empty }
        var entries: [String: (path: String, isDirectory: Bool)] = [:]
        for document in documents {
            let components = document.path.components(separatedBy: "/")
            guard components.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." }),
                  !document.path.contains("\\"), !document.path.contains(":"),
                  !document.path.contains("\0") else {
                throw MemoryExportError.invalidPath(document.path)
            }
            // ZIPs are commonly extracted onto case-insensitive filesystems.
            for index in components.indices {
                let path = components[...index].joined(separator: "/")
                let key = path.precomposedStringWithCanonicalMapping.lowercased()
                let isDirectory = index < components.count - 1
                if let existing = entries[key],
                   !isDirectory || !existing.isDirectory || existing.path != path {
                    throw MemoryExportError.conflictingPath(document.path)
                }
                entries[key] = (path, isDirectory)
            }
        }

        let fm = FileManager.default
        let temporary = fm.temporaryDirectory.appending(path: "clumsies-memory-\(UUID())")
        try fm.createDirectory(at: temporary, withIntermediateDirectories: false,
                               attributes: [.posixPermissions: 0o700])
        defer {
            do { try fm.removeItem(at: temporary) }
            catch { ClientDiagnostics.record("memory_export_cleanup_failed", ClientDiagnostics.failureFields(error)) }
        }
        let contents = temporary.appending(path: "files")
        try fm.createDirectory(at: contents, withIntermediateDirectories: false)
        for document in documents {
            let file = contents.appending(path: document.path)
            try fm.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data(document.body.utf8).write(to: file, options: .withoutOverwriting)
        }

        let archive = temporary.appending(path: "memory.zip")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = ["-c", "-k", "--norsrc", "--noextattr", "--noqtn", contents.path, archive.path]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        guard process.terminationReason == .exit, process.terminationStatus == 0 else {
            throw MemoryExportError.compressionFailed(process.terminationStatus)
        }
        // Publish only a complete ZIP; an earlier failure leaves any existing export intact.
        try Data(contentsOf: archive, options: .mappedIfSafe).write(to: destination, options: .atomic)
    }
}
