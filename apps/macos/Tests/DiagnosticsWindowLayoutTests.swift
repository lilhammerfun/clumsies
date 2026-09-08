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

}
