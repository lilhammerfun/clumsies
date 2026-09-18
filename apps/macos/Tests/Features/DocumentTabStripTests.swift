import AppKit
import XCTest
@testable import Clumsies

@MainActor
final class DocumentTabStripTests: XCTestCase {
    func testPreviewTitleDistinguishesPreviewFromSource() {
        let source = tab(title: "architecture.md", mode: .source)
        let preview = tab(title: "architecture.md", mode: .preview)

        XCTAssertEqual(DocumentTabPresentation.title(for: source), "architecture.md")
        XCTAssertEqual(DocumentTabPresentation.title(for: preview), "architecture.md Preview")
    }

    func testItemWidthClampsShortAndLongTitles() {
        let font = NSFont.systemFont(ofSize: NSFont.systemFontSize)
        let short = DocumentTabPresentation.itemWidth(for: tab(title: "a.md"), font: font)
        let long = DocumentTabPresentation.itemWidth(
            for: tab(title: String(repeating: "very-long-document-name-", count: 20)),
            font: font
        )

        XCTAssertEqual(short, DocumentTabMetrics.minimumItemWidth)
        XCTAssertEqual(long, DocumentTabMetrics.maximumItemWidth)
    }

    func testContentWidthIncludesHorizontalInsetsAndItemSpacing() {
        let tabs = [tab(title: "a.md"), tab(title: "b.md"), tab(title: "c.md")]
        let expected = DocumentTabMetrics.horizontalInset * 2
            + DocumentTabMetrics.minimumItemWidth * 3
            + DocumentTabMetrics.itemSpacing * 2

        XCTAssertEqual(DocumentTabPresentation.contentWidth(for: tabs), expected)
    }

    func testItemWidthsExpandEvenlyToFillAvailableWidth() {
        let tabs = [tab(title: "a.md"), tab(title: "architecture.md", itemId: "item-2")]
        let availableWidth = DocumentTabPresentation.contentWidth(for: tabs) + 100
        let spacing = DocumentTabMetrics.itemSpacing * CGFloat(tabs.count - 1)
        let expectedWidth = (
            availableWidth - DocumentTabMetrics.horizontalInset * 2 - spacing
        ) / CGFloat(tabs.count)

        let widths = DocumentTabPresentation.itemWidths(for: tabs, availableWidth: availableWidth)

        XCTAssertEqual(widths, Array(repeating: expectedWidth, count: tabs.count))
    }

    func testItemWidthsShrinkProportionallyToFitAvailableWidth() {
        let tabs = [
            tab(title: "architecture.md", itemId: "item-1"),
            tab(title: "PROJECT_DOCUMENT_ARCHITECTURE.md", itemId: "item-2"),
            tab(title: "rules.md", itemId: "item-3"),
        ]
        let availableWidth: CGFloat = 240
        let widths = DocumentTabPresentation.itemWidths(for: tabs, availableWidth: availableWidth)
        let spacing = DocumentTabMetrics.itemSpacing * CGFloat(tabs.count - 1)
        let occupiedWidth = DocumentTabMetrics.horizontalInset * 2 + spacing + widths.reduce(0, +)

        XCTAssertEqual(occupiedWidth, availableWidth, accuracy: 0.001)
        for (tab, width) in zip(tabs, widths) {
            XCTAssertLessThan(width, DocumentTabPresentation.itemWidth(for: tab))
        }
    }

    func testPreferredStripWidthCapsOverflowingTabs() {
        let tabs = (0..<20).map { tab(title: "document-\($0).md", itemId: "item-\($0)") }

        XCTAssertEqual(
            DocumentTabPresentation.preferredStripWidth(for: tabs),
            DocumentTabMetrics.maximumStripWidth
        )
    }

    func testCloseButtonAndTitleRemainInsideTabBounds() throws {
        let tab = tab(title: "architecture.md")
        let view = DocumentTabItemView(
            frame: NSRect(
                x: 0,
                y: 0,
                width: DocumentTabPresentation.itemWidth(for: tab),
                height: DocumentTabMetrics.itemHeight
            )
        )
        view.configure(title: DocumentTabPresentation.title(for: tab), onClose: {})
        view.layoutSubtreeIfNeeded()

        let closeButton = try XCTUnwrap(view.subviews.compactMap { $0 as? NSButton }.first)
        let titleLabel = try XCTUnwrap(view.subviews.compactMap { $0 as? NSTextField }.first)
        XCTAssertTrue(view.bounds.contains(closeButton.frame))
        XCTAssertGreaterThanOrEqual(titleLabel.frame.minX, closeButton.frame.maxX)
        XCTAssertLessThanOrEqual(titleLabel.frame.maxX, view.bounds.maxX)
        XCTAssertEqual(titleLabel.frame.midX, view.bounds.midX, accuracy: 0.001)
        XCTAssertEqual(titleLabel.alignment, .center)
    }

    func testSelectedTabUsesFilledStyleWithoutOutline() throws {
        let view = DocumentTabItemView(
            frame: NSRect(x: 0, y: 0, width: 120, height: DocumentTabMetrics.itemHeight)
        )
        view.isSelectedTab = true

        let layer = try XCTUnwrap(view.layer)
        let backgroundColor = try XCTUnwrap(layer.backgroundColor)
        XCTAssertEqual(layer.borderWidth, 0)
        XCTAssertGreaterThan(backgroundColor.alpha, 0)
    }

    func testLongTitleUsesTrailingFadeOnlyWhenClipped() throws {
        let label = DocumentTabTitleLabel(
            frame: NSRect(x: 0, y: 0, width: 60, height: DocumentTabMetrics.itemHeight)
        )
        label.font = .systemFont(ofSize: NSFont.systemFontSize)
        label.stringValue = "PROJECT_DOCUMENT_ARCHITECTURE.md"
        label.layoutSubtreeIfNeeded()
        XCTAssertTrue(label.layer?.mask is CAGradientLayer)

        label.frame.size.width = 400
        label.needsLayout = true
        label.layoutSubtreeIfNeeded()
        XCTAssertNil(label.layer?.mask)
    }

    func testTabFramesStayInsideStripContentBounds() throws {
        let tabs = [
            tab(title: "architecture.md", itemId: "item-1"),
            tab(title: "rules.md", itemId: "item-2"),
        ]
        let width = DocumentTabPresentation.preferredStripWidth(for: tabs)
        let view = DocumentTabStripView(
            frame: NSRect(x: 0, y: 0, width: width, height: DocumentTabMetrics.height)
        )
        view.update(tabs: tabs, selectedTabId: tabs[0].id, onSelect: { _ in }, onClose: { _ in })
        view.layoutSubtreeIfNeeded()

        let collectionView = try XCTUnwrap(view.subviews.compactMap { $0 as? NSCollectionView }.first)
        collectionView.layoutSubtreeIfNeeded()

        let firstFrame = try XCTUnwrap(
            collectionView.collectionViewLayout?.layoutAttributesForItem(
                at: IndexPath(item: 0, section: 0)
            )?.frame
        )
        let lastFrame = try XCTUnwrap(
            collectionView.collectionViewLayout?.layoutAttributesForItem(
                at: IndexPath(item: tabs.count - 1, section: 0)
            )?.frame
        )

        XCTAssertEqual(firstFrame.minX, DocumentTabMetrics.horizontalInset)
        XCTAssertEqual(firstFrame.minY, DocumentTabMetrics.verticalInset)
        XCTAssertEqual(
            lastFrame.maxX,
            collectionView.bounds.maxX - DocumentTabMetrics.horizontalInset,
            accuracy: 0.001
        )
        XCTAssertEqual(lastFrame.maxY, collectionView.bounds.maxY - DocumentTabMetrics.verticalInset)
    }

    func testOverflowingTabsShrinkInsideBoundsWithoutScrollView() throws {
        let tabs = (0..<20).map { tab(title: "document-\($0).md", itemId: "item-\($0)") }
        let view = DocumentTabStripView(
            frame: NSRect(
                x: 0,
                y: 0,
                width: DocumentTabMetrics.maximumStripWidth,
                height: DocumentTabMetrics.height
            )
        )
        view.update(tabs: tabs, selectedTabId: tabs[0].id, onSelect: { _ in }, onClose: { _ in })
        view.layoutSubtreeIfNeeded()

        XCTAssertFalse(view.subviews.contains { $0 is NSScrollView })
        let collectionView = try XCTUnwrap(view.subviews.compactMap { $0 as? NSCollectionView }.first)
        collectionView.layoutSubtreeIfNeeded()
        let firstFrame = try XCTUnwrap(
            collectionView.collectionViewLayout?.layoutAttributesForItem(
                at: IndexPath(item: 0, section: 0)
            )?.frame
        )
        let lastFrame = try XCTUnwrap(
            collectionView.collectionViewLayout?.layoutAttributesForItem(
                at: IndexPath(item: tabs.count - 1, section: 0)
            )?.frame
        )

        XCTAssertEqual(collectionView.frame, view.bounds)
        XCTAssertEqual(firstFrame.minX, DocumentTabMetrics.horizontalInset)
        XCTAssertEqual(
            lastFrame.maxX,
            collectionView.bounds.maxX - DocumentTabMetrics.horizontalInset,
            accuracy: 0.001
        )
    }

    func testResizingStripRecomputesCompressedTabFrames() throws {
        let tabs = (0..<8).map {
            tab(title: "PROJECT_DOCUMENT_ARCHITECTURE_\($0).md", itemId: "item-\($0)")
        }
        let view = DocumentTabStripView(
            frame: NSRect(x: 0, y: 0, width: 684.5, height: DocumentTabMetrics.height)
        )
        view.update(tabs: tabs, selectedTabId: tabs[0].id, onSelect: { _ in }, onClose: { _ in })
        view.layoutSubtreeIfNeeded()

        view.setFrameSize(NSSize(width: 384.5, height: DocumentTabMetrics.height))
        view.layoutSubtreeIfNeeded()

        let collectionView = try XCTUnwrap(view.subviews.compactMap { $0 as? NSCollectionView }.first)
        let firstFrame = try XCTUnwrap(
            collectionView.collectionViewLayout?.layoutAttributesForItem(
                at: IndexPath(item: 0, section: 0)
            )?.frame
        )
        let lastFrame = try XCTUnwrap(
            collectionView.collectionViewLayout?.layoutAttributesForItem(
                at: IndexPath(item: tabs.count - 1, section: 0)
            )?.frame
        )

        XCTAssertLessThan(firstFrame.width, DocumentTabMetrics.minimumItemWidth)
        XCTAssertEqual(firstFrame.minX, DocumentTabMetrics.horizontalInset)
        XCTAssertEqual(
            lastFrame.maxX,
            collectionView.bounds.maxX - DocumentTabMetrics.horizontalInset,
            accuracy: 0.001
        )
    }

    private func tab(
        title: String,
        itemId: String = "item-1",
        mode: WorkbenchTabMode = .source
    ) -> WorkbenchTab {
        WorkbenchTab(
            section: .memory,
            projectId: "project-1",
            itemId: itemId,
            mode: mode,
            title: title
        )
    }
}
