import AppKit
import SwiftUI

enum DocumentTabMetrics {
    static let height: CGFloat = 28
    static let horizontalInset: CGFloat = 3
    static let verticalInset: CGFloat = 1
    static let itemHeight: CGFloat = height - verticalInset * 2
    static let itemSpacing: CGFloat = 2
    static let minimumItemWidth: CGFloat = 84
    static let maximumItemWidth: CGFloat = 200
    static let maximumStripWidth: CGFloat = 560
    static let closeButtonWidth: CGFloat = 16
    static let leadingPadding: CGFloat = 5
    static let closeTitleSpacing: CGFloat = 4
    static let titleHorizontalInset = leadingPadding + closeButtonWidth + closeTitleSpacing
    static let itemCornerRadius: CGFloat = itemHeight / 2
    static let stripCornerRadius: CGFloat = height / 2
}

enum DocumentTabPresentation {
    static func title(for tab: WorkbenchTab) -> String {
        tab.mode == .preview ? "\(tab.title) Preview" : tab.title
    }

    static func itemWidth(
        for tab: WorkbenchTab,
        font: NSFont = .systemFont(ofSize: NSFont.systemFontSize)
    ) -> CGFloat {
        let titleWidth = (title(for: tab) as NSString).size(withAttributes: [.font: font]).width
        let chromeWidth = DocumentTabMetrics.titleHorizontalInset * 2
        return min(
            max(ceil(titleWidth) + chromeWidth, DocumentTabMetrics.minimumItemWidth),
            DocumentTabMetrics.maximumItemWidth
        )
    }

    static func contentWidth(for tabs: [WorkbenchTab]) -> CGFloat {
        guard !tabs.isEmpty else { return 0 }
        let itemWidths = tabs.reduce(CGFloat.zero) { partial, tab in
            partial + itemWidth(for: tab)
        }
        let spacing = DocumentTabMetrics.itemSpacing * CGFloat(tabs.count - 1)
        return DocumentTabMetrics.horizontalInset * 2 + itemWidths + spacing
    }

    static func itemWidths(
        for tabs: [WorkbenchTab],
        availableWidth: CGFloat,
        font: NSFont = .systemFont(ofSize: NSFont.systemFontSize)
    ) -> [CGFloat] {
        guard !tabs.isEmpty else { return [] }
        let preferredWidths = tabs.map { itemWidth(for: $0, font: font) }
        let spacing = DocumentTabMetrics.itemSpacing * CGFloat(tabs.count - 1)
        let availableItemWidth = max(
            0,
            availableWidth - DocumentTabMetrics.horizontalInset * 2 - spacing
        )
        let preferredTotal = preferredWidths.reduce(0, +)
        guard preferredTotal > 0 else { return [] }

        if preferredTotal <= availableItemWidth {
            let expandedWidth = availableItemWidth / CGFloat(tabs.count)
            return Array(repeating: expandedWidth, count: tabs.count)
        }

        let scale = availableItemWidth / preferredTotal
        var widths = preferredWidths.map { $0 * scale }
        if let lastIndex = widths.indices.last {
            let roundingCorrection = availableItemWidth - widths.reduce(0, +)
            widths[lastIndex] += roundingCorrection
        }
        return widths
    }

    static func preferredStripWidth(for tabs: [WorkbenchTab]) -> CGFloat {
        min(contentWidth(for: tabs), DocumentTabMetrics.maximumStripWidth)
    }
}

@MainActor
final class DocumentTabCollectionLayout: NSCollectionViewLayout {
    var itemWidths: ((CGFloat) -> [CGFloat])?

    private var cachedAttributes: [IndexPath: NSCollectionViewLayoutAttributes] = [:]
    private var cachedContentSize: NSSize = .zero

    override func prepare() {
        super.prepare()
        guard let collectionView else {
            cachedAttributes = [:]
            cachedContentSize = .zero
            return
        }

        let itemCount = collectionView.numberOfItems(inSection: 0)
        let widths = itemWidths?(collectionView.bounds.width) ?? []
        var attributes: [IndexPath: NSCollectionViewLayoutAttributes] = [:]
        var x = DocumentTabMetrics.horizontalInset

        for itemIndex in 0..<itemCount {
            let indexPath = IndexPath(item: itemIndex, section: 0)
            let itemAttributes = NSCollectionViewLayoutAttributes(forItemWith: indexPath)
            let width = widths.indices.contains(itemIndex) ? widths[itemIndex] : 0
            itemAttributes.frame = NSRect(
                x: x,
                y: DocumentTabMetrics.verticalInset,
                width: width,
                height: DocumentTabMetrics.itemHeight
            )
            attributes[indexPath] = itemAttributes
            x += width + DocumentTabMetrics.itemSpacing
        }

        cachedAttributes = attributes
        cachedContentSize = collectionView.bounds.size
    }

    override var collectionViewContentSize: NSSize {
        cachedContentSize
    }

    override func layoutAttributesForElements(
        in rect: NSRect
    ) -> [NSCollectionViewLayoutAttributes] {
        cachedAttributes.values.filter { $0.frame.intersects(rect) }
    }

    override func layoutAttributesForItem(
        at indexPath: IndexPath
    ) -> NSCollectionViewLayoutAttributes? {
        cachedAttributes[indexPath]
    }

    override func shouldInvalidateLayout(forBoundsChange newBounds: NSRect) -> Bool {
        collectionView?.bounds.size != newBounds.size
    }
}

struct DocumentTabStrip: NSViewRepresentable {
    let tabs: [WorkbenchTab]
    let selectedTabId: String?
    let onSelect: (WorkbenchTab) -> Void
    let onClose: (WorkbenchTab) -> Void

    func makeNSView(context: Context) -> DocumentTabStripView {
        let view = DocumentTabStripView()
        view.update(
            tabs: tabs,
            selectedTabId: selectedTabId,
            onSelect: onSelect,
            onClose: onClose
        )
        return view
    }

    func updateNSView(_ nsView: DocumentTabStripView, context: Context) {
        nsView.update(
            tabs: tabs,
            selectedTabId: selectedTabId,
            onSelect: onSelect,
            onClose: onClose
        )
    }
}

@MainActor
final class DocumentTabStripView: NSView {
    private let tabLayout = DocumentTabCollectionLayout()
    private let collectionView = NSCollectionView()
    private var tabs: [WorkbenchTab] = []
    private var selectedTabId: String?
    private var onSelect: ((WorkbenchTab) -> Void)?
    private var onClose: ((WorkbenchTab) -> Void)?
    private var isSynchronizingSelection = false

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        configureView()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        configureView()
    }

    override func setFrameSize(_ newSize: NSSize) {
        let sizeChanged = frame.size != newSize
        super.setFrameSize(newSize)
        if sizeChanged {
            updateCollectionLayoutForBounds()
        }
    }

    override var intrinsicContentSize: NSSize {
        return NSSize(
            width: DocumentTabPresentation.preferredStripWidth(for: tabs),
            height: DocumentTabMetrics.height
        )
    }

    override func layout() {
        super.layout()
        if collectionView.frame != bounds {
            updateCollectionLayoutForBounds()
        }
    }

    func update(
        tabs: [WorkbenchTab],
        selectedTabId: String?,
        onSelect: @escaping (WorkbenchTab) -> Void,
        onClose: @escaping (WorkbenchTab) -> Void
    ) {
        let contentChanged = self.tabs != tabs
        let selectionChanged = self.selectedTabId != selectedTabId
        self.tabs = tabs
        self.selectedTabId = selectedTabId
        self.onSelect = onSelect
        self.onClose = onClose

        if contentChanged {
            collectionView.reloadData()
            tabLayout.invalidateLayout()
            invalidateIntrinsicContentSize()
            needsLayout = true
        }
        if contentChanged || selectionChanged {
            synchronizeSelection()
        }
    }

    private func configureView() {
        wantsLayer = true
        layer?.cornerRadius = DocumentTabMetrics.stripCornerRadius
        layer?.masksToBounds = true

        setContentHuggingPriority(.defaultLow, for: .horizontal)
        setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        setContentHuggingPriority(.required, for: .vertical)
        setContentCompressionResistancePriority(.required, for: .vertical)
        setAccessibilityRole(.tabGroup)

        tabLayout.itemWidths = { [weak self] availableWidth in
            guard let self else { return [] }
            return DocumentTabPresentation.itemWidths(
                for: self.tabs,
                availableWidth: availableWidth
            )
        }
        collectionView.collectionViewLayout = tabLayout
        collectionView.backgroundColors = [.clear]
        collectionView.isSelectable = true
        collectionView.allowsMultipleSelection = false
        collectionView.allowsEmptySelection = false
        collectionView.dataSource = self
        collectionView.delegate = self
        collectionView.register(
            DocumentTabCollectionItem.self,
            forItemWithIdentifier: DocumentTabCollectionItem.identifier
        )
        collectionView.wantsLayer = true
        collectionView.layer?.masksToBounds = true
        addSubview(collectionView)
    }

    private func updateCollectionLayoutForBounds() {
        collectionView.frame = bounds
        tabLayout.invalidateLayout()
        tabLayout.prepare()
        collectionView.needsLayout = true
        collectionView.layoutSubtreeIfNeeded()
    }

    private func synchronizeSelection() {
        guard let selectedTabId,
              let index = tabs.firstIndex(where: { $0.id == selectedTabId }) else {
            collectionView.selectionIndexPaths = []
            return
        }

        let indexPath = IndexPath(item: index, section: 0)
        isSynchronizingSelection = true
        collectionView.selectionIndexPaths = [indexPath]
        isSynchronizingSelection = false
    }

}

extension DocumentTabStripView: NSCollectionViewDataSource {
    func collectionView(_ collectionView: NSCollectionView, numberOfItemsInSection section: Int) -> Int {
        tabs.count
    }

    func collectionView(
        _ collectionView: NSCollectionView,
        itemForRepresentedObjectAt indexPath: IndexPath
    ) -> NSCollectionViewItem {
        guard let item = collectionView.makeItem(
            withIdentifier: DocumentTabCollectionItem.identifier,
            for: indexPath
        ) as? DocumentTabCollectionItem else {
            preconditionFailure("Document tab item registration is invalid")
        }

        let tab = tabs[indexPath.item]
        item.configure(
            tab: tab,
            title: DocumentTabPresentation.title(for: tab),
            onClose: { [weak self] tab in self?.onClose?(tab) }
        )
        return item
    }
}

extension DocumentTabStripView: NSCollectionViewDelegate {
    func collectionView(_ collectionView: NSCollectionView, didSelectItemsAt indexPaths: Set<IndexPath>) {
        guard !isSynchronizingSelection,
              let indexPath = indexPaths.first,
              tabs.indices.contains(indexPath.item) else { return }
        onSelect?(tabs[indexPath.item])
    }
}

@MainActor
final class DocumentTabCollectionItem: NSCollectionViewItem {
    static let identifier = NSUserInterfaceItemIdentifier("DocumentTabCollectionItem")

    private var tabView: DocumentTabItemView {
        guard let tabView = view as? DocumentTabItemView else {
            preconditionFailure("Document tab item has an invalid view")
        }
        return tabView
    }

    override var isSelected: Bool {
        didSet { tabView.isSelectedTab = isSelected }
    }

    override func loadView() {
        view = DocumentTabItemView()
    }

    override func prepareForReuse() {
        super.prepareForReuse()
        tabView.prepareForReuse()
    }

    func configure(
        tab: WorkbenchTab,
        title: String,
        onClose: @escaping (WorkbenchTab) -> Void
    ) {
        tabView.configure(title: title, onClose: { onClose(tab) })
        tabView.isSelectedTab = isSelected
    }
}

@MainActor
final class DocumentTabTitleLabel: NSTextField {
    private let fadeMask = CAGradientLayer()
    private let fadeWidth: CGFloat = 18

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        isEditable = false
        isSelectable = false
        isBordered = false
        drawsBackground = false
        focusRingType = .none
        wantsLayer = true
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        isEditable = false
        isSelectable = false
        isBordered = false
        drawsBackground = false
        focusRingType = .none
        wantsLayer = true
    }

    override func layout() {
        super.layout()
        updateFadeMask()
    }

    private func updateFadeMask() {
        guard bounds.width > 0, let font else {
            layer?.mask = nil
            return
        }
        let textWidth = (stringValue as NSString).size(withAttributes: [.font: font]).width
        guard textWidth > bounds.width else {
            layer?.mask = nil
            return
        }

        let effectiveFadeWidth = min(fadeWidth, bounds.width / 2)
        let fadeStart = (bounds.width - effectiveFadeWidth) / bounds.width
        fadeMask.frame = bounds
        fadeMask.startPoint = CGPoint(x: 0, y: 0.5)
        fadeMask.endPoint = CGPoint(x: 1, y: 0.5)
        fadeMask.colors = [NSColor.black.cgColor, NSColor.black.cgColor, NSColor.clear.cgColor]
        fadeMask.locations = [0, NSNumber(value: Double(fadeStart)), 1]
        layer?.mask = fadeMask
    }
}

@MainActor
final class DocumentTabItemView: NSView {
    private let closeButton = NSButton()
    private let titleLabel = DocumentTabTitleLabel()
    private var trackingAreaReference: NSTrackingArea?
    private var onClose: (() -> Void)?
    private var isHovered = false {
        didSet { updateAppearance() }
    }

    var isSelectedTab = false {
        didSet { updateAppearance() }
    }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        configureView()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        configureView()
    }

    override func updateTrackingAreas() {
        if let trackingAreaReference {
            removeTrackingArea(trackingAreaReference)
        }
        let trackingArea = NSTrackingArea(
            rect: .zero,
            options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self
        )
        addTrackingArea(trackingArea)
        trackingAreaReference = trackingArea
        super.updateTrackingAreas()
    }

    override func layout() {
        super.layout()
        closeButton.frame = NSRect(
            x: DocumentTabMetrics.leadingPadding,
            y: floor((bounds.height - DocumentTabMetrics.closeButtonWidth) / 2),
            width: DocumentTabMetrics.closeButtonWidth,
            height: DocumentTabMetrics.closeButtonWidth
        )

        let titleHeight = min(titleLabel.intrinsicContentSize.height, bounds.height)
        let titleWidth = max(0, bounds.width - DocumentTabMetrics.titleHorizontalInset * 2)
        titleLabel.frame = NSRect(
            x: DocumentTabMetrics.titleHorizontalInset,
            y: floor((bounds.height - titleHeight) / 2),
            width: titleWidth,
            height: titleHeight
        )
    }

    override func mouseEntered(with event: NSEvent) {
        isHovered = true
    }

    override func mouseExited(with event: NSEvent) {
        isHovered = false
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        updateAppearance()
    }

    func configure(title: String, onClose: @escaping () -> Void) {
        titleLabel.stringValue = title
        titleLabel.needsLayout = true
        titleLabel.toolTip = title
        closeButton.toolTip = "Close \(title)"
        closeButton.setAccessibilityLabel("Close \(title)")
        setAccessibilityLabel(title)
        self.onClose = onClose
        needsLayout = true
    }

    override func prepareForReuse() {
        super.prepareForReuse()
        isHovered = false
        isSelectedTab = false
        onClose = nil
    }

    private func configureView() {
        wantsLayer = true
        layer?.cornerRadius = DocumentTabMetrics.itemCornerRadius
        layer?.cornerCurve = .continuous
        layer?.masksToBounds = true
        setAccessibilityElement(true)
        setAccessibilityRole(.button)

        closeButton.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close")?
            .withSymbolConfiguration(.init(pointSize: 8, weight: .medium))
        closeButton.imageScaling = .scaleProportionallyDown
        closeButton.isBordered = false
        closeButton.focusRingType = .none
        closeButton.contentTintColor = .secondaryLabelColor
        closeButton.target = self
        closeButton.action = #selector(closeTab)

        titleLabel.font = .systemFont(ofSize: NSFont.systemFontSize)
        titleLabel.textColor = .labelColor
        titleLabel.alignment = .center
        titleLabel.lineBreakMode = .byClipping
        titleLabel.maximumNumberOfLines = 1

        addSubview(closeButton)
        addSubview(titleLabel)
        updateAppearance()
    }

    private func updateAppearance() {
        if isSelectedTab {
            layer?.backgroundColor = NSColor.unemphasizedSelectedContentBackgroundColor.cgColor
            layer?.borderWidth = 0
            layer?.borderColor = nil
        } else if isHovered {
            layer?.backgroundColor = NSColor.labelColor.withAlphaComponent(0.07).cgColor
            layer?.borderWidth = 0
        } else {
            layer?.backgroundColor = NSColor.clear.cgColor
            layer?.borderWidth = 0
        }
        closeButton.alphaValue = isHovered ? 1 : 0
        closeButton.isEnabled = isHovered
        setAccessibilitySelected(isSelectedTab)
    }

    @objc private func closeTab() {
        onClose?()
    }
}
