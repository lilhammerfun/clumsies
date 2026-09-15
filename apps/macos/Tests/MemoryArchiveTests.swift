import XCTest
@testable import Clumsies

final class MemoryArchiveTests: XCTestCase {
    func testZIPRoundTripPreservesPathsAndUTF8WithoutExtraMetadata() throws {
        let temporary = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let destination = temporary.appending(path: "export.zip")
        let documents: [EditableMemoryDocument] = [
            .init(title: "Skill", path: "skills/中文/SKILL.md", body: "# 说明\n\n保留正文。\r\n"),
            .init(title: "Hidden", path: ".config/settings.json", body: "{\"enabled\":true}\n"),
            .init(title: "Empty", path: "empty.txt", body: ""),
        ]
        // Replacement must produce a fresh archive, not append to an older export.
        try MemoryArchive.write([.init(title: "Old", path: "old.md", body: "old")], to: destination)
        try MemoryArchive.write(documents, to: destination)
        XCTAssertEqual(try Data(contentsOf: destination).prefix(2), Data("PK".utf8))

        let extracted = temporary.appending(path: "extracted")
        let unzip = Process()
        unzip.executableURL = URL(fileURLWithPath: "/usr/bin/unzip")
        unzip.arguments = ["-qq", destination.path, "-d", extracted.path]
        try unzip.run()
        unzip.waitUntilExit()
        XCTAssertEqual(unzip.terminationStatus, 0)
        for document in documents {
            XCTAssertEqual(
                try Data(contentsOf: extracted.appending(path: document.path)),
                Data(document.body.utf8)
            )
        }
        let files = try XCTUnwrap(FileManager.default.enumerator(atPath: extracted.path))
            .allObjects.compactMap { $0 as? String }.filter {
                (try? extracted.appending(path: $0).resourceValues(forKeys: [.isRegularFileKey]))?
                    .isRegularFile == true
            }
        XCTAssertEqual(Set(files), Set(documents.map(\.path)))
    }

    func testUnsafeOrConflictingPathsFailWithoutReplacingDestination() throws {
        let temporary = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let destination = temporary.appending(path: "export.zip")
        let original = Data("existing export".utf8)
        try original.write(to: destination)
        let invalidPaths = ["", "../escape.md", "/absolute.md", "a/../../escape.md", "a//b.md",
                            "./file.md", "a/", "a\\b.md", "a:b.md", "nul\0.md"]
        let conflicts = [["a.md", "a.md"], ["A.md", "a.md"], ["a", "a/b.md"],
                         ["a/b.md", "a"], ["café.md", "cafe\u{301}.md"], ["Dir/a.md", "dir/b.md"]]
        for paths in [[]] + invalidPaths.map({ [$0] }) + conflicts {
            let documents = paths.map { EditableMemoryDocument(title: $0, path: $0, body: "text") }
            XCTAssertThrowsError(try MemoryArchive.write(documents, to: destination), "\(paths)")
            XCTAssertEqual(try Data(contentsOf: destination), original)
        }
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: temporary.path), ["export.zip"])
    }
}
