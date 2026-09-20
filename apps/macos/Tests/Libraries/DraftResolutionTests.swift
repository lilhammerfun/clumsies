import XCTest
@testable import Clumsies

final class DraftResolutionTests: XCTestCase {
    func testChoicesPreserveAutomaticChangesAndRequireEveryConflict() throws {
        let merged = "=======\n自动合并的标题\n<<<<<<<< ours\n远端一\n|||||||| original\n原文一\n========\n草稿一\n>>>>>>>> theirs\n中间\n<<<<<<<< ours\n远端二\n|||||||| original\n原文二\n========\n草稿二\n>>>>>>>> theirs\n自动合并的结尾\n"
        let candidate = fixture(content: merged, conflicts: ["content"])
        var resolution = DraftResolution(candidate: candidate)
        XCTAssertFalse(resolution.canSave)
        XCTAssertFalse(resolution.canEditContent)
        XCTAssertEqual(resolution.sections.count, 2)
        XCTAssertFalse(resolution.previewText.contains("<<<<<<<< ours"))
        let first = try XCTUnwrap(resolution.sections.first)
        resolution.chooseContent(first.shared, in: first)
        XCTAssertFalse(resolution.canSave)
        XCTAssertEqual(resolution.sections.count, 1)
        let second = try XCTUnwrap(resolution.sections.first)
        resolution.chooseContent(second.proposed, in: second)
        XCTAssertTrue(resolution.canSave)
        XCTAssertTrue(resolution.canEditContent)
        XCTAssertEqual(resolution.text, "=======\n自动合并的标题\n远端一\n中间\n草稿二\n自动合并的结尾\n")
        XCTAssertEqual(resolution.path, "remote-rename.md")
        resolution.editContent(resolution.text + "手动编辑\n")
        XCTAssertTrue(resolution.canSave)
        let saved = resolution
        resolution.chooseContent(first.proposed, in: first)
        XCTAssertEqual(resolution, saved, "an obsolete section must not overwrite edited text")
        resolution.editContent(merged)
        XCTAssertFalse(resolution.canSave)
    }

    func testPathAndDeletionNeedExplicitChoicesAndDoNotReplaceMergedContent() {
        let candidate = fixture(content: "自动合并", conflicts: ["path"])
        var resolution = DraftResolution(candidate: candidate)
        XCTAssertFalse(resolution.canSave)
        resolution.choosePath(" ")
        XCTAssertFalse(resolution.canSave)
        resolution.choosePath("chosen.md")
        XCTAssertTrue(resolution.canSave)
        XCTAssertEqual(resolution.text, "自动合并")
        var deletion = DraftResolution(candidate: fixture(content: "保留内容", conflicts: ["exists"]))
        XCTAssertFalse(deletion.canSave)
        deletion.chooseFile(.init(exists: false, resource: candidate.draftState.resource, content: nil))
        XCTAssertTrue(deletion.canSave)
        XCTAssertFalse(deletion.state.exists)
        XCTAssertNil(deletion.state.content)
    }

    func testMissingPreviewNeverSilentlyTreatsDraftAsResolved() {
        var candidate = fixture(content: "草稿", conflicts: ["content"])
        candidate.mergePreview = nil
        var resolution = DraftResolution(candidate: candidate)
        XCTAssertFalse(resolution.canSave)
        resolution.editContent("some edit")
        XCTAssertFalse(resolution.canSave)
        resolution.chooseFile(candidate.currentState)
        XCTAssertTrue(resolution.canSave)
        XCTAssertEqual(resolution.state, candidate.currentState)
    }

    private func fixture(content: String, conflicts: [String]) -> DraftReconciliationCandidate {
        let ref = ServerDraftResourceReference(scope: "org", id: "memory", path: "remote-rename.md")
        let state = ReconciliationResourceState(exists: true, resource: ref,
            content: .init(description: nil, content: content))
        return .init(candidateId: "candidate", draftId: "draft", draftVersion: 1,
            baseCommitId: "base", currentCommitId: "remote", status: .conflicts,
            baseState: state, currentState: state, draftState: state, proposedState: nil,
            conflicts: conflicts.map { .init(kind: $0, field: $0, base: nil, current: nil, draft: nil) },
            resultHash: nil, valid: true, createdAt: "2026-09-20T00:00:00Z", invalidatedAt: nil,
            mergePreview: .init(state: state, markerLength: 8))
    }
}
