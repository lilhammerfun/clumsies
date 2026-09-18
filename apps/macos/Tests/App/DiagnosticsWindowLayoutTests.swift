import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

@MainActor
final class DiagnosticsWindowLayoutTests: XCTestCase {
    func testReadableRunListWidthIsAHardMinimum() {
        XCTAssertEqual(
            RetrievalDiagnosticsLayout.runListMinimumWidth,
            RetrievalDiagnosticsLayout.runListIdealWidth
        )
        XCTAssertGreaterThanOrEqual(RetrievalDiagnosticsLayout.runListMinimumWidth, 360)
    }

    func testEvidenceReviewExposesOnlyOneContextualPrimaryAction() {
        XCTAssertEqual(
            RetrievalEvidenceReviewAction(
                hasSelection: false,
                canRecordNoMatch: true
            ),
            .noMatch
        )
        XCTAssertEqual(
            RetrievalEvidenceReviewAction(
                hasSelection: false,
                canRecordNoMatch: true
            ).title,
            "No Match"
        )
        XCTAssertEqual(
            RetrievalEvidenceReviewAction(
                hasSelection: true,
                canRecordNoMatch: true
            ),
            .confirm
        )
        XCTAssertEqual(
            RetrievalEvidenceReviewAction(
                hasSelection: true,
                canRecordNoMatch: false
            ).title,
            "Confirm"
        )
        XCTAssertEqual(
            RetrievalEvidenceReviewAction(
                hasSelection: false,
                canRecordNoMatch: false
            ),
            .done
        )
    }

    func testCandidateFiltersIncludeReusedSelectionsAndPreserveOrder() throws {
        let selected = try retrievalDetail(runId: "selected", selected: true).candidates[0]
        let excluded = try retrievalDetail(runId: "excluded", selected: false).candidates[0]
        let candidates = [excluded, selected]

        XCTAssertEqual(candidates.filter(RetrievalCandidateFilter.all.includes), candidates)
        XCTAssertEqual(candidates.filter(RetrievalCandidateFilter.selected.includes), [selected])
        XCTAssertEqual(candidates.filter(RetrievalCandidateFilter.excluded.includes), [excluded])
        XCTAssertEqual(selected.deltaAction, .reuse)
    }

    func testRunSelectionIgnoresLateResultsAndClearsLoadingImmediately() async throws {
        let firstStarted = expectation(description: "First run requested")
        let secondStarted = expectation(description: "Second run requested")
        let thirdStarted = expectation(description: "Third run requested")
        let started = ["first": firstStarted, "second": secondStarted, "third": thirdStarted]
        var pending: [String: CheckedContinuation<RetrievalRunDetail, Error>] = [:]
        let model = RetrievalDiagnosticsModel(daemon: DaemonXPCClient(), fetchRun: { runId in
            try await withCheckedThrowingContinuation { continuation in
                pending[runId] = continuation
                started[runId]?.fulfill()
            }
        })

        let first = Task { await model.select(runId: "first") }
        await fulfillment(of: [firstStarted], timeout: 1)
        let second = Task { await model.select(runId: "second") }
        await fulfillment(of: [secondStarted], timeout: 1)
        XCTAssertTrue(model.isLoading)
        XCTAssertNil(model.detail)

        try XCTUnwrap(pending.removeValue(forKey: "second"))
            .resume(returning: retrievalDetail(runId: "second"))
        await second.value
        try XCTUnwrap(pending.removeValue(forKey: "first"))
            .resume(returning: retrievalDetail(runId: "first"))
        await first.value
        XCTAssertEqual(model.selectedRunId, "second")
        XCTAssertEqual(model.detail?.run.runId, "second")
        XCTAssertFalse(model.isLoading)
        XCTAssertTrue(model.runs.isEmpty)

        let third = Task { await model.select(runId: "third") }
        await fulfillment(of: [thirdStarted], timeout: 1)
        await model.select(runId: nil)
        XCTAssertNil(model.selectedRunId)
        XCTAssertNil(model.detail)
        XCTAssertFalse(model.isLoading)
        try XCTUnwrap(pending.removeValue(forKey: "third"))
            .resume(throwing: URLError(.resourceUnavailable))
        await third.value
        XCTAssertNil(model.errorMessage)
        XCTAssertNil(model.detail)
    }

    func testUnavailableRunCanRetryTheSameSelection() async throws {
        var attempts = 0
        let loaded = try retrievalDetail(runId: "retry")
        let model = RetrievalDiagnosticsModel(daemon: DaemonXPCClient(), fetchRun: { _ in
            attempts += 1
            if attempts == 1 { throw URLError(.resourceUnavailable) }
            return loaded
        })

        await model.select(runId: "retry")
        XCTAssertEqual(model.selectedRunId, "retry")
        XCTAssertNil(model.detail)
        XCTAssertNotNil(model.errorMessage)
        XCTAssertFalse(model.isLoading)

        await model.select(runId: "retry")
        XCTAssertEqual(attempts, 2)
        XCTAssertEqual(model.detail?.run.runId, "retry")
        XCTAssertNil(model.errorMessage)
        XCTAssertFalse(model.isLoading)
    }

    private func retrievalDetail(
        runId: String,
        selected: Bool = true
    ) throws -> RetrievalRunDetail {
        let json = """
        {
          "run": {
            "run_id": "\(runId)", "project_id": "project-1", "query": "test query",
            "activation_state_fingerprint": "state", "status": "succeeded",
            "resource_count": 1, "unit_count": 1,
            "latencies": {
              "effective_memory_us": 1, "index_ensure_us": 1, "bm25_us": 1,
              "embedding_us": 1, "vector_us": 1, "rrf_us": 1, "rerank_us": 1,
              "assembly_us": 1, "persistence_us": 1, "total_us": 9
            },
            "returned_fragment_count": 0, "returned_token_count": 0,
            "created_at": "2026-09-08T00:00:00Z"
          },
          "candidates": [{
            "unit_key": "\(runId)-unit", "resource_id": "resource-1",
            "scope": "project", "kind": "memory", "path": "memory.md",
            "heading_path": [],
            "locator": {"type": "markdown_span", "start_byte": 0,
                        "end_byte": 8, "heading_path": []},
            "content_hash": "content", "resource_content_hash": "resource",
            "token_count": 2, "evidence_excerpt": "evidence",
            "selected": \(selected),
            "exclusion_reason": "\(selected ? "selected" : "not_reranked")",
            "delta_action": \(selected ? "\"reuse\"" : "null")
          }],
          "evidence": [], "evidence_suggestions": []
        }
        """
        return try JSONCoding.decoder().decode(RetrievalRunDetail.self, from: Data(json.utf8))
    }

}
