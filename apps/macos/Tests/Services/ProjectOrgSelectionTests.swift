import XCTest
@testable import Clumsies

final class ProjectOrgSelectionTests: XCTestCase {
    func testAddPreservesExistingMemoryAndAddsEveryRequestedResource() {
        let current: Set<String> = ["context-a", "rule-a"]
        let requested: Set<String> = ["context-a", "workflow-a"]

        let result = ProjectOrgSelectionMutation.add.applying(requested, to: current)

        XCTAssertEqual(result, ["context-a", "rule-a", "workflow-a"])
    }

    func testRemoveOnlyRemovesRequestedResourcesThatAreCurrentlySelected() {
        let current: Set<String> = ["context-a", "rule-a", "workflow-a"]
        let requested: Set<String> = ["context-a", "context-missing"]

        let result = ProjectOrgSelectionMutation.remove.applying(requested, to: current)

        XCTAssertEqual(result, ["rule-a", "workflow-a"])
    }

    func testRepeatedAddIsIdempotent() {
        let current: Set<String> = ["context-a"]

        let result = ProjectOrgSelectionMutation.add.applying(["context-a"], to: current)

        XCTAssertEqual(result, current)
    }
}
