import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

final class FileTreeSelectionTests: XCTestCase {
    func testSharedPathTreeBuildsDirectoriesForReviewAndMemoryNavigators() throws {
        let roots = PathTreeNode.build([
            PathTreeItem(id: "review-file", path: "context/guides/review.md"),
            PathTreeItem(id: "memory-file", path: "context/notes.md"),
        ])

        let context = try XCTUnwrap(roots.first)
        XCTAssertEqual(context.id, "directory:context")
        XCTAssertEqual(context.name, "context")
        XCTAssertEqual(PathTreeNode.firstItemId(in: roots), "review-file")
        XCTAssertEqual(
            PathTreeNode.directoryIds(in: roots),
            ["directory:context", "directory:context/guides"]
        )
        XCTAssertNotNil(PathTreeNode.node(withId: "review-file", in: roots)?.item)
    }

    func testUnifiedDiffGutterFitsLineNumbersAndAlignsNestedContent() {
        let short = UnifiedDiffMetrics.lineGutterWidth(maximumLineNumber: 99)
        let long = UnifiedDiffMetrics.lineGutterWidth(maximumLineNumber: 99_999)

        XCTAssertEqual(short, 30)
        XCTAssertEqual(long, 48)
        XCTAssertEqual(
            UnifiedDiffMetrics.contentLeadingInset(
                lineGutterWidth: short,
                showsRemoteLineNumbers: false
            ),
            104
        )
        XCTAssertEqual(
            UnifiedDiffMetrics.contentLeadingInset(
                lineGutterWidth: short,
                showsRemoteLineNumbers: true
            ),
            134
        )

        let lines = (1...1_500).map { lineNumber in
            UnifiedDiffLine(
                id: "line-\(lineNumber)",
                kind: lineNumber == 750 ? .insertion : .context,
                text: "line \(lineNumber)",
                oldLineNumber: lineNumber,
                newLineNumber: lineNumber
            )
        }
        XCTAssertEqual(UnifiedDiffPresentation(lines: lines).maximumLineNumber, 1_500)
    }

    func testInlineDiffCommentsUseViewportWidthInsteadOfLongCodeWidth() {
        XCTAssertEqual(
            UnifiedDiffMetrics.inlineCommentWidth(
                viewportWidth: 900,
                leadingInset: 104
            ),
            588
        )
        XCTAssertEqual(
            UnifiedDiffMetrics.inlineCommentWidth(
                viewportWidth: 2_000,
                leadingInset: 104
            ),
            760
        )
        XCTAssertEqual(
            UnifiedDiffMetrics.inlineCommentWidth(
                viewportWidth: 440,
                leadingInset: 104
            ),
            320
        )
        XCTAssertEqual(
            UnifiedDiffMetrics.inlineCommentWidth(
                viewportWidth: 200,
                leadingInset: 104
            ),
            84
        )
    }

    @MainActor
    func testUnifiedDiffBackgroundsDoNotInheritRoundedContainerShape() throws {
        let presentation = UnifiedDiffPresentation(model: .make(
            original: "Checkout at 12:00.", modified: "Checkout at 13:00."
        ))
        func render(cornerRadius: CGFloat) throws -> NSBitmapImageRep {
            let host = NSHostingView(rootView:
                UnifiedDiffView(presentation: presentation)
                    .frame(width: 400, height: 74, alignment: .topLeading)
                    .background(.white, in: RoundedRectangle(cornerRadius: cornerRadius))
                    .environment(\.colorScheme, .light)
            )
            host.frame = NSRect(x: 0, y: 0, width: 400, height: 74)
            let window = NSWindow(contentRect: host.frame, styleMask: [], backing: .buffered, defer: false)
            window.contentView = host
            for _ in 0..<3 {
                host.layoutSubtreeIfNeeded()
                RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            }
            let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: bitmap)
            return bitmap
        }
        let plain = try render(cornerRadius: 0)
        let rounded = try render(cornerRadius: 8)
        func color(_ bitmap: NSBitmapImageRep, x: Int, y: Int) throws -> NSColor {
            try XCTUnwrap(bitmap.colorAt(x: x * bitmap.pixelsWide / 400,
                                        y: y * bitmap.pixelsHigh / 74)?.usingColorSpace(.deviceRGB))
        }
        let removal = try color(plain, x: 80, y: 38)
        let insertion = try color(plain, x: 80, y: 62)
        XCTAssertGreaterThan(removal.redComponent, removal.greenComponent)
        XCTAssertGreaterThan(insertion.greenComponent, insertion.redComponent)
        // Sample the hunk, changed rows, and line-number gutters at their edges.
        for y in [1, 27, 51] {
            for x in [1, 29, 31, 59, 398] {
                let expected = try color(plain, x: x, y: y)
                let actual = try color(rounded, x: x, y: y)
                XCTAssertEqual(actual.alphaComponent, 1, accuracy: 0.005)
                XCTAssertEqual(actual.redComponent, expected.redComponent, accuracy: 0.005, "x=\(x), y=\(y)")
                XCTAssertEqual(actual.greenComponent, expected.greenComponent, accuracy: 0.005, "x=\(x), y=\(y)")
                XCTAssertEqual(actual.blueComponent, expected.blueComponent, accuracy: 0.005, "x=\(x), y=\(y)")
            }
        }
    }

    @MainActor
    func testDeletionPlaceholderKeepsDocumentTabsAtTheTopWhenResizing() throws {
        let workspace = WorkspaceCoordinator()
        workspace.context.phase = .ready
        workspace.context.projects = [.init(id: "project", name: "Project", refCommitId: "base", refEtag: "base",
            selectedOrgResourceIds: ["memory"], orgSelectionRevision: 0, isLoaded: true)]
        workspace.context.activeProjectId = "project"
        workspace.catalog.resources = [try XCTUnwrap(memoryItem(id: "memory", path: "deleted.md").resource)]
        workspace.edits.drafts = [draft(targetId: "memory", isDeletion: true, scope: .org)]
        let item = try XCTUnwrap(workspace.navigation.memoryItems.first)
        workspace.navigation.open(item, mode: .source)
        let host = NSHostingView(rootView: MemoryMainPane()
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .workspaceEnvironment(workspace))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 700),
            styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        func tabStrip(in view: NSView) -> DocumentTabStripView? {
            if let strip = view as? DocumentTabStripView { return strip }
            return view.subviews.lazy.compactMap { tabStrip(in: $0) }.first
        }
        for height in [CGFloat(500), 820] {
            window.setContentSize(NSSize(width: 900, height: height))
            for _ in 0..<3 {
                host.layoutSubtreeIfNeeded()
                RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            }
            let strip = try XCTUnwrap(tabStrip(in: host))
            let frame = strip.convert(strip.bounds, to: host)
            let topInset = host.isFlipped ? frame.minY : host.bounds.maxY - frame.maxY
            XCTAssertEqual(topInset, 0, accuracy: 1, "The deletion placeholder must not center the entire document pane.")
            XCTAssertEqual(frame.height, DocumentTabMetrics.height, accuracy: 1)
        }
    }

    @MainActor
    func testLongUnifiedDiffLineCreatesHorizontalScrollRangeAfterHunkHeader() throws {
        let presentation = UnifiedDiffPresentation(lines: [
            .init(
                id: "long-insertion",
                kind: .insertion,
                text: String(repeating: "x", count: 300),
                oldLineNumber: nil,
                newLineNumber: 1
            )
        ])
        let root = HSplitView {
            Color.clear
                .frame(minWidth: 180, idealWidth: 220, maxWidth: 280, maxHeight: .infinity)
            ScrollView(.vertical) {
                VStack(alignment: .leading, spacing: 20) {
                    Text("Header")
                    UnifiedDiffView(presentation: presentation)
                }
                .frame(maxWidth: 1180, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .top)
                .padding(.horizontal, 24)
            }
            .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(width: 900, height: 500)

        let host = NSHostingView(rootView: root)
        host.frame = NSRect(x: 0, y: 0, width: 900, height: 500)
        let window = NSWindow(
            contentRect: host.frame,
            styleMask: [.titled, .resizable],
            backing: .buffered,
            defer: false
        )
        window.contentView = host

        for _ in 0..<3 {
            host.layoutSubtreeIfNeeded()
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        }

        let inner = try XCTUnwrap(
            descendantScrollViews(in: host).first {
                $0.hasHorizontalScroller && !$0.hasVerticalScroller
            }
        )
        let documentWidth = try XCTUnwrap(inner.documentView).frame.width
        XCTAssertGreaterThan(documentWidth, inner.contentView.bounds.width + 1)
    }

    @MainActor
    func testUnifiedDiffRowsStayCompactInsideFullHeightContainer() throws {
        let presentation = UnifiedDiffPresentation(lines: (1...3).map { (lineNumber: Int) in
            .init(
                id: "line-\(lineNumber)",
                kind: .insertion,
                text: "line \(lineNumber)",
                oldLineNumber: nil,
                newLineNumber: lineNumber
            )
        })
        let root = GeometryReader { geometry in
            ScrollView(.vertical) {
                VStack(alignment: .leading, spacing: 0) {
                    UnifiedDiffView(presentation: presentation)
                }
                .frame(
                    maxWidth: .infinity,
                    minHeight: geometry.size.height,
                    alignment: .topLeading
                )
            }
        }
        .frame(width: 800, height: 500)

        let host = NSHostingView(rootView: root)
        host.frame = NSRect(x: 0, y: 0, width: 800, height: 500)
        let window = NSWindow(
            contentRect: host.frame,
            styleMask: [.titled, .resizable],
            backing: .buffered,
            defer: false
        )
        window.contentView = host

        for _ in 0..<3 {
            host.layoutSubtreeIfNeeded()
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        }

        let inner = try XCTUnwrap(
            descendantScrollViews(in: host).first {
                $0.hasHorizontalScroller && !$0.hasVerticalScroller
            }
        )
        XCTAssertEqual(inner.frame.height, 98, accuracy: 1)
    }

    @MainActor
    func testCleanReconciliationKeepsUnifiedDiffRenderer() throws {
        let resource = ServerDraftResourceReference(
            scope: "project",
            id: "memory",
            path: "workflow/SYNC_REAL.md"
        )
        let baseState = ReconciliationResourceState(
            exists: true,
            resource: resource,
            content: .init(description: nil, content: "remote: base\nlocal: base")
        )
        let draftState = ReconciliationResourceState(
            exists: true,
            resource: resource,
            content: .init(description: nil, content: "remote: base\nlocal: draft")
        )
        let currentState = ReconciliationResourceState(
            exists: true,
            resource: resource,
            content: .init(description: nil, content: "remote: shared\nlocal: base")
        )
        func candidate(
            status: DraftReconciliationStatus,
            proposedState: ReconciliationResourceState?,
            conflicts: [ReconciliationConflict]
        ) -> DraftReconciliationCandidate {
            .init(
                candidateId: "candidate",
                draftId: "draft",
                draftVersion: 1,
                baseCommitId: "base",
                currentCommitId: "current",
                status: status,
                baseState: baseState,
                currentState: currentState,
                draftState: draftState,
                proposedState: proposedState,
                conflicts: conflicts,
                resultHash: "result",
                valid: true,
                createdAt: "2026-08-31T00:00:00Z",
                invalidatedAt: nil
            )
        }

        let cleanCandidate = candidate(
            status: .clean,
            proposedState: .init(
                exists: true,
                resource: resource,
                content: .init(
                    description: nil,
                    content: "remote: shared\nlocal: draft"
                )
            ),
            conflicts: []
        )
        let states = cleanCandidate.postSyncDiffStates
        let changedLines = UnifiedDiffPresentation(
            model: SplitDiffModel.make(
                original: states.base.content?.primaryText ?? "",
                modified: states.draft.content?.primaryText ?? "",
                contextLineCount: 0
            )
        ).blocks.flatMap(\.lines).filter { $0.kind.isChanged }
        XCTAssertEqual(changedLines.map(\.kind), [.removal, .insertion])
        XCTAssertEqual(changedLines.map(\.text), ["local: base", "local: draft"])

        let candidates = [cleanCandidate]

        for candidate in candidates {
            let root = DraftReconciliationView(
                candidate: candidate,
                usesContextualUpdateAction: true,
                onCancel: {},
                onApply: { _ in }
            )
            .frame(width: 800, height: 500)

            let host = NSHostingView(rootView: root)
            host.frame = NSRect(x: 0, y: 0, width: 800, height: 500)
            let window = NSWindow(
                contentRect: host.frame,
                styleMask: [.titled, .resizable],
                backing: .buffered,
                defer: false
            )
            window.contentView = host

            for _ in 0..<3 {
                host.layoutSubtreeIfNeeded()
                RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            }

            XCTAssertNotNil(
                descendantScrollViews(in: host).first {
                    $0.hasHorizontalScroller && !$0.hasVerticalScroller
                }
            )
        }
    }

    func testPlainDirectoryClickSelectsAndTogglesDirectory() {
        let result = FileTreeSelectionInteraction.directoryClick(
            nodeId: "directory:design",
            visibleNodeIds: ["directory:design", "a", "b"],
            currentSelection: ["a"],
            anchorId: "a",
            modifierFlags: []
        )

        XCTAssertEqual(result.selection, ["directory:design"])
        XCTAssertEqual(result.anchorId, "directory:design")
        XCTAssertTrue(result.togglesDirectory)
    }

    @MainActor
    private func descendantScrollViews(in view: NSView) -> [NSScrollView] {
        let current = (view as? NSScrollView).map { [$0] } ?? []
        return current + view.subviews.flatMap(descendantScrollViews)
    }

    func testShiftClickDirectorySelectsRangeWithoutTogglingDirectory() {
        let result = FileTreeSelectionInteraction.directoryClick(
            nodeId: "directory:other",
            visibleNodeIds: ["a", "b", "directory:other", "c"],
            currentSelection: ["a"],
            anchorId: "a",
            modifierFlags: .shift
        )

        XCTAssertEqual(result.selection, ["a", "b", "directory:other"])
        XCTAssertEqual(result.anchorId, "a")
        XCTAssertFalse(result.togglesDirectory)
    }

    func testCommandClickDirectoryTogglesSelectionWithoutTogglingDirectory() {
        let result = FileTreeSelectionInteraction.directoryClick(
            nodeId: "directory:design",
            visibleNodeIds: ["directory:design", "a"],
            currentSelection: ["a"],
            anchorId: "a",
            modifierFlags: .command
        )

        XCTAssertEqual(result.selection, ["a", "directory:design"])
        XCTAssertEqual(result.anchorId, "directory:design")
        XCTAssertFalse(result.togglesDirectory)
    }

    func testDirectorySelectionExpandsToEveryDescendantWithoutDuplicates() {
        let items = [
            memoryItem(id: "a", path: "design/a.md"),
            memoryItem(id: "b", path: "design/nested/b.md"),
            memoryItem(id: "c", path: "other/c.md"),
        ]
        let roots = FileTreeNode.build(items)

        let selected = FileTreeNode.items(
            in: roots,
            selectedNodeIds: ["directory:design", "a"]
        )

        XCTAssertEqual(selected.map(\.id), ["a", "b"])
    }

    func testSingleFileDirectoryKeepsDirectoryIdentity() throws {
        let roots = FileTreeNode.build([
            memoryItem(id: "only", path: "notes/only.md"),
        ])

        let directory = try XCTUnwrap(FileTreeNode.selectedDirectory(
            in: roots,
            selectedNodeIds: ["directory:notes"]
        ))

        XCTAssertEqual(directory.id, "directory:notes")
        XCTAssertNil(directory.item)
        XCTAssertEqual(
            FileTreeNode.items(
                in: roots,
                selectedNodeIds: [directory.id]
            ).map(\.id),
            ["only"]
        )
    }

    func testFileSelectionDoesNotIncludeSiblingFiles() {
        let items = [
            memoryItem(id: "a", path: "design/a.md"),
            memoryItem(id: "b", path: "design/b.md"),
        ]
        let roots = FileTreeNode.build(items)

        let selected = FileTreeNode.items(
            in: roots,
            selectedNodeIds: ["b"]
        )

        XCTAssertEqual(selected.map(\.id), ["b"])
    }

    func testNilItemUsesPrimaryTone() {
        XCTAssertEqual(MemoryFileTreeTitleTone.resolve(item: nil), .primary)
    }

    func testSyncedResourceUsesPrimaryTone() {
        let item = MemoryListItem(id: "res", resource: nil, draft: nil, inherited: false)
        XCTAssertEqual(MemoryFileTreeTitleTone.resolve(item: item), .primary)
    }

    func testInheritedResourceWithoutDraftUsesPrimaryTone() {
        let item = MemoryListItem(id: "res", resource: nil, draft: nil, inherited: true)
        XCTAssertEqual(MemoryFileTreeTitleTone.resolve(item: item), .primary)
    }

    func testNewDraftUsesGreenToneRegardlessOfInheritance() {
        for inherited in [false, true] {
            let item = MemoryListItem(
                id: "new",
                resource: nil,
                draft: draft(targetId: nil),
                inherited: inherited
            )
            XCTAssertEqual(MemoryFileTreeTitleTone.resolve(item: item), .newDraft)
        }
    }

    func testModifiedDraftUsesYellowToneRegardlessOfInheritance() {
        for inherited in [false, true] {
            let item = MemoryListItem(
                id: "res",
                resource: nil,
                draft: draft(targetId: "res"),
                inherited: inherited
            )
            XCTAssertEqual(MemoryFileTreeTitleTone.resolve(item: item), .modifiedDraft)
        }
    }

    func testDeletionDraftUsesRedToneRegardlessOfInheritance() {
        for inherited in [false, true] {
            let item = MemoryListItem(
                id: "res",
                resource: nil,
                draft: draft(targetId: "res", isDeletion: true),
                inherited: inherited
            )
            XCTAssertEqual(MemoryFileTreeTitleTone.resolve(item: item), .deletedDraft)
        }
    }

    func testSubmittedChangesKeepTheirColorsUntilMergedOrDiscarded() {
        for status in [DaemonLocalDraftStatus.open, .submitted, .merged, .discarded] {
            for (targetId, isDeletion, tone) in [
                (String?.none, false, MemoryFileTreeTitleTone.newDraft),
                (Optional("memory"), false, .modifiedDraft),
                (Optional("memory"), true, .deletedDraft),
            ] {
                let item = MemoryListItem(id: "memory", resource: nil,
                    draft: draft(targetId: targetId, isDeletion: isDeletion, status: status), inherited: true)
                XCTAssertEqual(MemoryFileTreeTitleTone.resolve(item: item),
                    status == .open || status == .submitted ? tone : .primary)
            }
        }
    }

    @MainActor
    func testDraftReviewIconsAreBundledAndReadable() throws {
        for name in ["git-pull-request-16", "git-pull-request-draft-16"] {
            let url = try XCTUnwrap(Bundle.main.url(
                forResource: name, withExtension: "svg", subdirectory: "Octicons"
            ))
            let image = try XCTUnwrap(NSImage(contentsOf: url))
            XCTAssertEqual(image.size, NSSize(width: 16, height: 16))
        }
    }

    func testThreeWayLocalChangeShowsRemovalThenInsertion() {
        let lines = ThreeWayDiff.lines(base: "a\nb", local: "a\nB", remote: "a\nb")
        XCTAssertEqual(lines.map(\.kind), [.context, .removal, .insertion])
    }

    func testThreeWayRemoteChangeShowsGrayLines() {
        let lines = ThreeWayDiff.lines(base: "a\nb", local: "a\nb", remote: "a\nB")
        XCTAssertEqual(lines.map(\.kind), [.context, .remoteRemoval, .remoteInsertion])
    }

    func testThreeWayConflictShowsRemoteThenLocal() {
        let lines = ThreeWayDiff.lines(base: "a", local: "L", remote: "R")
        XCTAssertEqual(lines.map(\.kind), [.removal, .remoteInsertion, .insertion])
    }

    func testThreeWayIdenticalLocalAndRemoteReplacementIsEmittedOnce() {
        let lines = ThreeWayDiff.lines(base: "a", local: "x", remote: "x")

        XCTAssertEqual(lines.map(\.kind), [.removal, .insertion])
        XCTAssertEqual(lines.map(\.text), ["a", "x"])
        XCTAssertEqual(lines.map(\.oldLineNumber), [1, nil])
        XCTAssertEqual(lines.map(\.newLineNumber), [nil, 1])
        XCTAssertEqual(lines.map(\.remoteLineNumber), [nil, 1])
    }

    func testThreeWayMultiLineConflictKeepsRemoteAndLocalGroupsContiguous() {
        let lines = ThreeWayDiff.lines(
            base: "old 1\nold 2",
            local: "local 1\nlocal 2",
            remote: "remote 1\nremote 2"
        )

        XCTAssertEqual(lines.map(\.kind), [
            .removal,
            .removal,
            .remoteInsertion,
            .remoteInsertion,
            .insertion,
            .insertion,
        ])
        XCTAssertEqual(lines.map(\.text), [
            "old 1",
            "old 2",
            "remote 1",
            "remote 2",
            "local 1",
            "local 2",
        ])
        XCTAssertEqual(lines.compactMap(\.remoteLineNumber), [1, 2])
        XCTAssertEqual(lines.compactMap(\.newLineNumber), [1, 2])
    }

    func testThreeWayConflictEmitsSharedPrefixAndSuffixOnce() {
        let lines = ThreeWayDiff.lines(
            base: "old prefix\nold middle\nold suffix",
            local: "shared prefix\nlocal middle\nshared suffix",
            remote: "shared prefix\nremote middle\nshared suffix"
        )

        XCTAssertEqual(lines.map(\.kind), [
            .removal,
            .removal,
            .removal,
            .insertion,
            .remoteInsertion,
            .insertion,
            .insertion,
        ])
        XCTAssertEqual(lines.map(\.text), [
            "old prefix",
            "old middle",
            "old suffix",
            "shared prefix",
            "remote middle",
            "local middle",
            "shared suffix",
        ])
        XCTAssertEqual(lines.compactMap(\.newLineNumber), [1, 2, 3])
        XCTAssertEqual(lines.compactMap(\.remoteLineNumber), [1, 2, 3])
        XCTAssertEqual(lines.filter { $0.text == "shared prefix" }.count, 1)
        XCTAssertEqual(lines.filter { $0.text == "shared suffix" }.count, 1)
    }

    func testThreeWayEmptyBaseYieldsPureInsertions() {
        let lines = ThreeWayDiff.lines(base: "", local: "a\nb", remote: "")
        XCTAssertEqual(lines.map(\.kind), [.insertion, .insertion])
    }

    func testThreeWayEmptyBaseWithRemoteYieldsGrayFirst() {
        let lines = ThreeWayDiff.lines(base: "", local: "a", remote: "b")
        XCTAssertEqual(lines.map(\.kind), [.remoteInsertion, .insertion])
    }

    func testThreeWayPureRemoteInsertionFromEmptyBaseUsesRemoteCoordinatesAndLabel() throws {
        let lines = ThreeWayDiff.lines(base: "", local: "", remote: "remote 1\nremote 2")

        XCTAssertEqual(lines.map(\.kind), [.remoteInsertion, .remoteInsertion])
        XCTAssertEqual(lines.map(\.text), ["remote 1", "remote 2"])
        XCTAssertEqual(lines.map(\.oldLineNumber), [nil, nil])
        XCTAssertEqual(lines.map(\.newLineNumber), [nil, nil])
        XCTAssertEqual(lines.map(\.remoteLineNumber), [1, 2])

        let presentation = UnifiedDiffPresentation(lines: lines)
        XCTAssertTrue(presentation.showsRemoteLineNumbers)
        let block = try XCTUnwrap(presentation.blocks.first)
        guard case .hunk(let label) = block.kind else {
            return XCTFail("Expected a three-way hunk")
        }
        XCTAssertEqual(label, "@@ base 0,0 · local 0,0 · remote 1,2 @@")
    }

    func testThreeWayHunkLabelKeepsInsertionBoundaryForEmptyAxes() throws {
        let lines = ThreeWayDiff.lines(
            base: "a\nb\nc",
            local: "a\nx\nb\nc",
            remote: "a\nb\nc"
        )

        let presentation = UnifiedDiffPresentation(lines: lines, contextLineCount: 0)
        let block = try XCTUnwrap(presentation.blocks.first { block in
            if case .hunk = block.kind { return true }
            return false
        })
        guard case .hunk(let label) = block.kind else {
            return XCTFail("Expected a three-way hunk")
        }
        XCTAssertEqual(label, "@@ base 1,0 · local 2,1 · remote 1,0 @@")
    }

    func testThreeWayHunkLabelKeepsDeletionBoundaryForEmptyAxis() throws {
        let lines = ThreeWayDiff.lines(
            base: "a\nb\nc",
            local: "a\nc",
            remote: "a\nb\nc"
        )

        let presentation = UnifiedDiffPresentation(lines: lines, contextLineCount: 0)
        let block = try XCTUnwrap(presentation.blocks.first { block in
            if case .hunk = block.kind { return true }
            return false
        })
        guard case .hunk(let label) = block.kind else {
            return XCTFail("Expected a three-way hunk")
        }
        XCTAssertEqual(label, "@@ base 2,1 · local 1,0 · remote 2,1 @@")
    }

    func testThreeWayRemoteReplacementPreservesTextAndThreeCoordinateSpaces() throws {
        let lines = ThreeWayDiff.lines(base: "a\nb", local: "a\nb", remote: "a\nB")

        XCTAssertEqual(lines.map(\.text), ["a", "b", "B"])
        XCTAssertEqual(lines.map(\.oldLineNumber), [1, 2, nil])
        XCTAssertEqual(lines.map(\.newLineNumber), [1, 2, nil])
        XCTAssertEqual(lines.map(\.remoteLineNumber), [1, nil, 2])

        let presentation = UnifiedDiffPresentation(lines: lines)
        let block = try XCTUnwrap(presentation.blocks.first)
        guard case .hunk(let label) = block.kind else {
            return XCTFail("Expected a three-way hunk")
        }
        XCTAssertEqual(label, "@@ base 1,2 · local 1,2 · remote 1,2 @@")
    }

    func testThreeWayDiffRetainsRepeatedBaseLines() {
        let lines = ThreeWayDiff.lines(
            base: "repeat\nrepeat\nend",
            local: "repeat\nlocal\nrepeat\nend",
            remote: "repeat\nremote\nrepeat\nend"
        )
        let context = lines.filter { $0.kind == .context }

        XCTAssertEqual(context.map(\.text), ["repeat", "repeat", "end"])
        XCTAssertEqual(context.compactMap(\.oldLineNumber), [1, 2, 3])
        XCTAssertEqual(
            lines.filter { $0.kind == .remoteInsertion }.map(\.text),
            ["remote"]
        )
        XCTAssertEqual(
            lines.filter { $0.kind == .insertion }.map(\.text),
            ["local"]
        )
    }

    func testTwoWayUnifiedPresentationKeepsStandardHeaderAndTwoCoordinates() throws {
        let presentation = UnifiedDiffPresentation(model: SplitDiffModel.make(
            original: "old",
            modified: "new"
        ))

        XCTAssertFalse(presentation.showsRemoteLineNumbers)
        let block = try XCTUnwrap(presentation.blocks.first)
        guard case .hunk(let label) = block.kind else {
            return XCTFail("Expected a two-way hunk")
        }
        XCTAssertEqual(label, "@@ -1,1 +1,1 @@")
        XCTAssertTrue(block.lines.allSatisfy { $0.remoteLineNumber == nil })
    }

    func testThreeWayUnchangedStreamYieldsNoBlocks() {
        let presentation = UnifiedDiffPresentation(lines: ThreeWayDiff.lines(
            base: "a", local: "a", remote: "a"
        ))
        XCTAssertEqual(presentation.changedLineCount, 0)
    }

    private func draft(
        targetId: String?,
        isDeletion: Bool = false,
        status: DaemonLocalDraftStatus = .open,
        scope: MemoryScope = .project
    ) -> LocalDraft {
        LocalDraft(
            id: "draft-\(targetId ?? "new")",
            projectId: "project",
            serverId: nil,
            serverVersion: 0,
            baseCommitId: "base",
            currentCommitId: "base",
            freshness: .current,
            hasUpstreamResourceChanges: false,
            reconciliation: .unknown,
            reconciliationCandidateId: nil,
            scope: scope,
            kind: .context,
            targetId: targetId,
            status: status,
            origin: .desktop,
            syncStatus: .synced,
            updatedAt: "2026-08-05T00:00:00Z",
            document: .init(title: "Untitled", path: "a.md", body: ""),
            isDeletion: isDeletion
        )
    }

    private func memoryItem(
        id: String,
        path: String,
        scope: MemoryScope = .org,
        projectId: String? = nil
    ) -> MemoryListItem {
        .init(
            id: id,
            resource: .init(
                id: id,
                scope: scope,
                projectId: projectId,
                projectName: nil,
                kind: .context,
                contentHash: "sha256:\(id)",
                updatedAt: "2026-07-31T00:00:00Z",
                refCommitId: "commit",
                contentLoaded: true,
                document: .init(
                    title: URL(fileURLWithPath: path).lastPathComponent,
                    path: path,
                    body: ""
                )
            ),
            draft: nil,
            inherited: false
        )
    }
}
