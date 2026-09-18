import SwiftUI

enum UnifiedDiffLineKind: Equatable, Sendable {
    case context
    case insertion
    case removal
    case remoteInsertion
    case remoteRemoval

    var isChanged: Bool { self != .context }
}

struct UnifiedDiffLine: Identifiable, Equatable, Sendable {
    let id: String
    let kind: UnifiedDiffLineKind
    let text: String
    let oldLineNumber: Int?
    let newLineNumber: Int?
    let remoteLineNumber: Int?

    init(
        id: String,
        kind: UnifiedDiffLineKind,
        text: String,
        oldLineNumber: Int?,
        newLineNumber: Int?,
        remoteLineNumber: Int? = nil
    ) {
        self.id = id
        self.kind = kind
        self.text = text
        self.oldLineNumber = oldLineNumber
        self.newLineNumber = newLineNumber
        self.remoteLineNumber = remoteLineNumber
    }

    var commentAnchorLine: Int? {
        kind == .removal ? nil : newLineNumber
    }
}

struct UnifiedDiffBlockPresentation: Identifiable, Equatable, Sendable {
    let id: Int
    let kind: SplitDiffBlockKind
    let lines: [UnifiedDiffLine]
}

struct UnifiedDiffPresentation: Equatable, Sendable {
    let blocks: [UnifiedDiffBlockPresentation]
    let showsRemoteLineNumbers: Bool
    let maximumLineNumber: Int

    init(model: SplitDiffModel, anchoredLines: Set<Int> = []) {
        showsRemoteLineNumbers = false
        if model.blocks.isEmpty, !anchoredLines.isEmpty {
            let resolvedBlocks = Self.anchoredContextBlocks(
                rows: model.rows,
                anchoredLines: anchoredLines,
                startingBlockId: 0
            )
            blocks = resolvedBlocks
            maximumLineNumber = Self.maximumLineNumber(in: resolvedBlocks)
            return
        }

        var presentations: [UnifiedDiffBlockPresentation] = []
        for block in model.blocks {
            if case .omission = block.kind,
               block.rows.contains(where: { row in
                   guard let line = row.modified?.lineNumber else { return false }
                   return anchoredLines.contains(line)
               }) {
                presentations.append(contentsOf: Self.anchoredContextBlocks(
                    rows: block.rows,
                    anchoredLines: anchoredLines,
                    startingBlockId: presentations.count
                ))
                continue
            }

            let id = presentations.count
            presentations.append(UnifiedDiffBlockPresentation(
                id: id,
                kind: block.kind,
                lines: Self.lines(for: block.rows, blockId: id)
            ))
        }
        blocks = presentations
        maximumLineNumber = Self.maximumLineNumber(in: presentations)
    }

    var changedLineCount: Int {
        blocks.reduce(into: 0) { count, block in
            guard case .hunk = block.kind else { return }
            count += block.lines.count
        }
    }

    private static func maximumLineNumber(
        in blocks: [UnifiedDiffBlockPresentation]
    ) -> Int {
        blocks.reduce(into: 0) { result, block in
            for line in block.lines {
                result = max(result, line.oldLineNumber ?? 0)
                result = max(result, line.newLineNumber ?? 0)
                result = max(result, line.remoteLineNumber ?? 0)
            }
        }
    }

    private static func lines(
        for rows: [SplitDiffRow],
        blockId: Int
    ) -> [UnifiedDiffLine] {
        rows.flatMap { row in
            var lines: [UnifiedDiffLine] = []

            if let original = row.original, original.kind == .removal {
                lines.append(UnifiedDiffLine(
                    id: "\(blockId)-\(row.id)-removal",
                    kind: .removal,
                    text: original.text,
                    oldLineNumber: original.lineNumber,
                    newLineNumber: nil
                ))
            }

            if let modified = row.modified, modified.kind == .insertion {
                lines.append(UnifiedDiffLine(
                    id: "\(blockId)-\(row.id)-insertion",
                    kind: .insertion,
                    text: modified.text,
                    oldLineNumber: nil,
                    newLineNumber: modified.lineNumber
                ))
            } else if let modified = row.modified {
                lines.append(UnifiedDiffLine(
                    id: "\(blockId)-\(row.id)-context",
                    kind: .context,
                    text: modified.text,
                    oldLineNumber: row.original?.lineNumber,
                    newLineNumber: modified.lineNumber
                ))
            }

            return lines
        }
    }

    private static func anchoredContextBlocks(
        rows: [SplitDiffRow],
        anchoredLines: Set<Int>,
        startingBlockId: Int,
        contextLineCount: Int = 3
    ) -> [UnifiedDiffBlockPresentation] {
        let anchorIndices = rows.indices.filter { index in
            guard let line = rows[index].modified?.lineNumber else { return false }
            return anchoredLines.contains(line)
        }
        guard !anchorIndices.isEmpty else { return [] }

        var ranges: [ClosedRange<Int>] = []
        for index in anchorIndices {
            let candidate = max(0, index - contextLineCount)
                ... min(rows.count - 1, index + contextLineCount)
            if let previous = ranges.last,
               candidate.lowerBound <= previous.upperBound + 1 {
                ranges[ranges.count - 1] = previous.lowerBound
                    ... max(previous.upperBound, candidate.upperBound)
            } else {
                ranges.append(candidate)
            }
        }

        var result: [UnifiedDiffBlockPresentation] = []
        var cursor = 0
        for range in ranges {
            if cursor < range.lowerBound {
                appendBlock(
                    kind: .omission,
                    rows: Array(rows[cursor..<range.lowerBound]),
                    startingBlockId: startingBlockId,
                    to: &result
                )
            }

            let hunkRows = Array(rows[range])
            appendBlock(
                kind: .hunk(hunkLabel(for: hunkRows)),
                rows: hunkRows,
                startingBlockId: startingBlockId,
                to: &result
            )
            cursor = range.upperBound + 1
        }

        if cursor < rows.count {
            appendBlock(
                kind: .omission,
                rows: Array(rows[cursor...]),
                startingBlockId: startingBlockId,
                to: &result
            )
        }
        return result
    }

    private static func appendBlock(
        kind: SplitDiffBlockKind,
        rows: [SplitDiffRow],
        startingBlockId: Int,
        to blocks: inout [UnifiedDiffBlockPresentation]
    ) {
        let id = startingBlockId + blocks.count
        blocks.append(.init(
            id: id,
            kind: kind,
            lines: lines(for: rows, blockId: id)
        ))
    }

    private static func hunkLabel(for rows: [SplitDiffRow]) -> String {
        let oldStart = rows.first?.original?.lineNumber ?? rows.first?.modified?.lineNumber ?? 1
        let newStart = rows.first?.modified?.lineNumber ?? rows.first?.original?.lineNumber ?? 1
        let oldCount = rows.filter { $0.original != nil }.count
        let newCount = rows.filter { $0.modified != nil }.count
        return "@@ -\(oldStart),\(oldCount) +\(newStart),\(newCount) @@"
    }
}

/// Builds a unified diff stream from three full texts.
///
/// - base -> local changes render green/red (the user's own changes).
/// - base -> remote changes render gray (other people's committed changes
///   that are not part of the local draft yet).
enum ThreeWayDiff: Sendable {
    private struct VersionLine: Equatable, Sendable {
        let text: String
        let lineNumber: Int
    }

    private struct Edit: Sendable {
        let baseRange: Range<Int>
        let replacement: [VersionLine]
    }

    private struct SideDiff: Sendable {
        let edits: [Edit]
        let unchangedLineNumbers: [Int: Int]

        func lineNumber(forBaseIndex index: Int) -> Int {
            unchangedLineNumbers[index] ?? index + 1
        }

        func applying(
            _ selectedEdits: [Edit],
            to baseRange: Range<Int>,
            baseLines: [String]
        ) -> [VersionLine] {
            var result: [VersionLine] = []
            var cursor = baseRange.lowerBound

            for edit in selectedEdits {
                if cursor < edit.baseRange.lowerBound {
                    result.append(contentsOf: (cursor ..< edit.baseRange.lowerBound).map { index in
                        VersionLine(
                            text: baseLines[index],
                            lineNumber: lineNumber(forBaseIndex: index)
                        )
                    })
                }
                result.append(contentsOf: edit.replacement)
                cursor = max(cursor, edit.baseRange.upperBound)
            }

            if cursor < baseRange.upperBound {
                result.append(contentsOf: (cursor ..< baseRange.upperBound).map { index in
                    VersionLine(
                        text: baseLines[index],
                        lineNumber: lineNumber(forBaseIndex: index)
                    )
                })
            }
            return result
        }
    }

    private enum Side: Sendable {
        case local
        case remote
    }

    private struct TaggedEdit: Sendable {
        let side: Side
        let edit: Edit
    }

    private struct EditRegion: Sendable {
        var baseRange: Range<Int>
        var localEdits: [Edit] = []
        var remoteEdits: [Edit] = []

        mutating func add(_ taggedEdit: TaggedEdit) {
            baseRange = min(baseRange.lowerBound, taggedEdit.edit.baseRange.lowerBound)
                ..< max(baseRange.upperBound, taggedEdit.edit.baseRange.upperBound)
            switch taggedEdit.side {
            case .local:
                localEdits.append(taggedEdit.edit)
            case .remote:
                remoteEdits.append(taggedEdit.edit)
            }
        }
    }

    static func lines(base: String, local: String, remote: String) -> [UnifiedDiffLine] {
        let baseLines = split(base)
        let localDiff = sideDiff(base: base, version: local)
        let remoteDiff = sideDiff(base: base, version: remote)
        let regions = editRegions(local: localDiff.edits, remote: remoteDiff.edits)

        var lines: [UnifiedDiffLine] = []
        var counter = 0
        func emit(
            _ kind: UnifiedDiffLineKind,
            _ text: String,
            old: Int?,
            new: Int?,
            remote: Int?
        ) {
            counter += 1
            lines.append(.init(
                id: "three-way-\(counter)",
                kind: kind,
                text: text,
                oldLineNumber: old,
                newLineNumber: new,
                remoteLineNumber: remote
            ))
        }

        func emitContext(_ range: Range<Int>) {
            for index in range {
                emit(
                    .context,
                    baseLines[index],
                    old: index + 1,
                    new: localDiff.lineNumber(forBaseIndex: index),
                    remote: remoteDiff.lineNumber(forBaseIndex: index)
                )
            }
        }

        func emitRegion(_ region: EditRegion) {
            let localChanged = !region.localEdits.isEmpty
            let remoteChanged = !region.remoteEdits.isEmpty
            let localResult = localDiff.applying(
                region.localEdits,
                to: region.baseRange,
                baseLines: baseLines
            )
            let remoteResult = remoteDiff.applying(
                region.remoteEdits,
                to: region.baseRange,
                baseLines: baseLines
            )

            if localChanged, remoteChanged {
                for index in region.baseRange {
                    emit(.removal, baseLines[index], old: index + 1, new: nil, remote: nil)
                }

                let maximumPrefixCount = min(localResult.count, remoteResult.count)
                var sharedPrefixCount = 0
                while sharedPrefixCount < maximumPrefixCount,
                      localResult[sharedPrefixCount].text == remoteResult[sharedPrefixCount].text {
                    sharedPrefixCount += 1
                }

                let maximumSuffixCount = maximumPrefixCount - sharedPrefixCount
                var sharedSuffixCount = 0
                while sharedSuffixCount < maximumSuffixCount,
                      localResult[localResult.count - sharedSuffixCount - 1].text
                        == remoteResult[remoteResult.count - sharedSuffixCount - 1].text {
                    sharedSuffixCount += 1
                }

                for offset in 0 ..< sharedPrefixCount {
                    let localLine = localResult[offset]
                    let remoteLine = remoteResult[offset]
                    emit(
                        .insertion,
                        localLine.text,
                        old: nil,
                        new: localLine.lineNumber,
                        remote: remoteLine.lineNumber
                    )
                }

                let remoteMiddleEnd = remoteResult.count - sharedSuffixCount
                for line in remoteResult[sharedPrefixCount ..< remoteMiddleEnd] {
                    emit(
                        .remoteInsertion,
                        line.text,
                        old: nil,
                        new: nil,
                        remote: line.lineNumber
                    )
                }

                let localMiddleEnd = localResult.count - sharedSuffixCount
                for line in localResult[sharedPrefixCount ..< localMiddleEnd] {
                    emit(
                        .insertion,
                        line.text,
                        old: nil,
                        new: line.lineNumber,
                        remote: nil
                    )
                }

                for offset in 0 ..< sharedSuffixCount {
                    let localLine = localResult[localMiddleEnd + offset]
                    let remoteLine = remoteResult[remoteMiddleEnd + offset]
                    emit(
                        .insertion,
                        localLine.text,
                        old: nil,
                        new: localLine.lineNumber,
                        remote: remoteLine.lineNumber
                    )
                }
                return
            }

            if localChanged {
                for index in region.baseRange {
                    emit(
                        .removal,
                        baseLines[index],
                        old: index + 1,
                        new: nil,
                        remote: remoteDiff.lineNumber(forBaseIndex: index)
                    )
                }
                for line in localResult {
                    emit(
                        .insertion,
                        line.text,
                        old: nil,
                        new: line.lineNumber,
                        remote: nil
                    )
                }
                return
            }

            for index in region.baseRange {
                emit(
                    .remoteRemoval,
                    baseLines[index],
                    old: index + 1,
                    new: localDiff.lineNumber(forBaseIndex: index),
                    remote: nil
                )
            }
            for line in remoteResult {
                emit(
                    .remoteInsertion,
                    line.text,
                    old: nil,
                    new: nil,
                    remote: line.lineNumber
                )
            }
        }

        var baseCursor = 0
        for region in regions {
            if baseCursor < region.baseRange.lowerBound {
                emitContext(baseCursor ..< region.baseRange.lowerBound)
            }
            emitRegion(region)
            baseCursor = max(baseCursor, region.baseRange.upperBound)
        }
        if baseCursor < baseLines.count {
            emitContext(baseCursor ..< baseLines.count)
        }
        return lines
    }

    private static func sideDiff(base: String, version: String) -> SideDiff {
        let rows = SplitDiffModel.make(original: base, modified: version).rows
        var edits: [Edit] = []
        var unchangedLineNumbers: [Int: Int] = [:]
        var baseOffset = 0
        var rowIndex = 0

        while rowIndex < rows.count {
            let row = rows[rowIndex]
            if !row.isChanged {
                if let original = row.original, let modified = row.modified {
                    unchangedLineNumbers[original.lineNumber - 1] = modified.lineNumber
                    baseOffset = original.lineNumber
                }
                rowIndex += 1
                continue
            }

            let start = baseOffset
            var replacement: [VersionLine] = []
            while rowIndex < rows.count, rows[rowIndex].isChanged {
                let changedRow = rows[rowIndex]
                if let original = changedRow.original {
                    baseOffset = original.lineNumber
                }
                if let modified = changedRow.modified {
                    replacement.append(.init(
                        text: modified.text,
                        lineNumber: modified.lineNumber
                    ))
                }
                rowIndex += 1
            }
            edits.append(.init(baseRange: start ..< baseOffset, replacement: replacement))
        }

        return .init(edits: edits, unchangedLineNumbers: unchangedLineNumbers)
    }

    private static func editRegions(local: [Edit], remote: [Edit]) -> [EditRegion] {
        var tagged = local.map { TaggedEdit(side: .local, edit: $0) }
        tagged.append(contentsOf: remote.map { TaggedEdit(side: .remote, edit: $0) })
        tagged.sort { lhs, rhs in
            if lhs.edit.baseRange.lowerBound != rhs.edit.baseRange.lowerBound {
                return lhs.edit.baseRange.lowerBound < rhs.edit.baseRange.lowerBound
            }
            if lhs.edit.baseRange.upperBound != rhs.edit.baseRange.upperBound {
                return lhs.edit.baseRange.upperBound < rhs.edit.baseRange.upperBound
            }
            switch (lhs.side, rhs.side) {
            case (.remote, .local): return true
            default: return false
            }
        }

        var regions: [EditRegion] = []
        for taggedEdit in tagged {
            if let lastIndex = regions.indices.last,
               overlaps(regions[lastIndex].baseRange, taggedEdit.edit.baseRange) {
                regions[lastIndex].add(taggedEdit)
            } else {
                var region = EditRegion(baseRange: taggedEdit.edit.baseRange)
                region.add(taggedEdit)
                regions.append(region)
            }
        }
        return regions
    }

    private static func overlaps(_ lhs: Range<Int>, _ rhs: Range<Int>) -> Bool {
        if lhs.isEmpty, rhs.isEmpty {
            return lhs.lowerBound == rhs.lowerBound
        }
        if lhs.isEmpty {
            return lhs.lowerBound > rhs.lowerBound && lhs.lowerBound < rhs.upperBound
        }
        if rhs.isEmpty {
            return rhs.lowerBound > lhs.lowerBound && rhs.lowerBound < lhs.upperBound
        }
        return lhs.overlaps(rhs)
    }

    private static func split(_ text: String) -> [String] {
        guard !text.isEmpty else { return [] }
        return text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    }
}

extension UnifiedDiffPresentation {
    /// Builds a presentation from a fully expanded unified line stream,
    /// collapsing unchanged runs into omissions like `init(model:)` does.
    init(lines: [UnifiedDiffLine], contextLineCount: Int = 3) {
        showsRemoteLineNumbers = true
        let changedIndices = lines.indices.filter { lines[$0].kind.isChanged }
        guard !changedIndices.isEmpty else {
            blocks = []
            maximumLineNumber = 0
            return
        }
        let context = max(0, contextLineCount)
        var ranges: [ClosedRange<Int>] = []
        for index in changedIndices {
            let candidate = max(0, index - context) ... min(lines.count - 1, index + context)
            if let previous = ranges.last, candidate.lowerBound <= previous.upperBound + 1 {
                ranges[ranges.count - 1] = previous.lowerBound ... max(previous.upperBound, candidate.upperBound)
            } else {
                ranges.append(candidate)
            }
        }

        var result: [UnifiedDiffBlockPresentation] = []
        var cursor = 0
        for range in ranges {
            if cursor < range.lowerBound {
                result.append(.init(
                    id: result.count,
                    kind: .omission,
                    lines: Array(lines[cursor ..< range.lowerBound])
                ))
            }
            let hunkLines = Array(lines[range])
            let precedingLines = lines[..<range.lowerBound]
            result.append(.init(
                id: result.count,
                kind: .hunk(Self.threeWayHunkLabel(
                    hunkLines,
                    baseBefore: precedingLines.compactMap(\.oldLineNumber).last ?? 0,
                    localBefore: precedingLines.compactMap(\.newLineNumber).last ?? 0,
                    remoteBefore: precedingLines.compactMap(\.remoteLineNumber).last ?? 0
                )),
                lines: hunkLines
            ))
            cursor = range.upperBound + 1
        }
        if cursor < lines.count {
            result.append(.init(
                id: result.count,
                kind: .omission,
                lines: Array(lines[cursor...])
            ))
        }
        blocks = result
        maximumLineNumber = Self.maximumLineNumber(in: result)
    }

    private static func threeWayHunkLabel(
        _ lines: [UnifiedDiffLine],
        baseBefore: Int,
        localBefore: Int,
        remoteBefore: Int
    ) -> String {
        let base = coordinateLabel(
            "base",
            numbers: lines.compactMap(\.oldLineNumber),
            before: baseBefore
        )
        let local = coordinateLabel(
            "local",
            numbers: lines.compactMap(\.newLineNumber),
            before: localBefore
        )
        let remote = coordinateLabel(
            "remote",
            numbers: lines.compactMap(\.remoteLineNumber),
            before: remoteBefore
        )
        return "@@ \(base) · \(local) · \(remote) @@"
    }

    private static func coordinateLabel(
        _ name: String,
        numbers: [Int],
        before: Int
    ) -> String {
        guard let first = numbers.first else { return "\(name) \(before),0" }
        return "\(name) \(first),\(numbers.count)"
    }
}

enum UnifiedDiffMetrics {
    static let minimumLineGutterWidth: CGFloat = 30
    static let markerWidth: CGFloat = 18
    static let commentButtonWidth: CGFloat = 26
    static let minimumInlineCommentWidth: CGFloat = 320
    static let inlineCommentWidthRatio: CGFloat = 0.75

    static func lineGutterWidth(maximumLineNumber: Int) -> CGFloat {
        let digits = String(max(1, maximumLineNumber)).count
        return max(minimumLineGutterWidth, CGFloat(digits * 8 + 8))
    }

    static func contentLeadingInset(
        lineGutterWidth: CGFloat,
        showsRemoteLineNumbers: Bool
    ) -> CGFloat {
        lineGutterWidth * (showsRemoteLineNumbers ? 3 : 2)
            + markerWidth
            + commentButtonWidth
    }

    static func inlineCommentWidth(
        viewportWidth: CGFloat,
        leadingInset: CGFloat
    ) -> CGFloat {
        let availableWidth = max(viewportWidth - leadingInset - 12, 1)
        let preferredWidth = max(
            minimumInlineCommentWidth,
            availableWidth * inlineCommentWidthRatio
        )
        return min(availableWidth, DocumentContentMetrics.maximumWidth, preferredWidth)
    }
}

private struct UnifiedDiffViewportWidthKey: PreferenceKey {
    static let defaultValue: CGFloat = 0

    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = max(value, nextValue())
    }
}

struct UnifiedDiffView: View {
    let presentation: UnifiedDiffPresentation
    let commentsByLine: [Int: [ReviewComment]]
    let composingLine: Int?
    @Binding var commentDraft: String
    let isSubmittingComment: Bool
    let showsCommentControls: Bool
    let onRequestComment: (Int) -> Void
    let onCancelComment: () -> Void
    let onSubmitComment: (Int) -> Void
    let onReply: (Int) -> Void

    @State private var hoveredLineId: String?
    @State private var expandedOmissionIds: Set<Int> = []
    @State private var viewportWidth: CGFloat = 0

    init(
        model: SplitDiffModel,
        commentsByLine: [Int: [ReviewComment]],
        composingLine: Int?,
        commentDraft: Binding<String>,
        isSubmittingComment: Bool,
        onRequestComment: @escaping (Int) -> Void,
        onCancelComment: @escaping () -> Void,
        onSubmitComment: @escaping (Int) -> Void,
        onReply: @escaping (Int) -> Void
    ) {
        var anchoredLines = Set(commentsByLine.keys)
        if let composingLine {
            anchoredLines.insert(composingLine)
        }
        presentation = UnifiedDiffPresentation(
            model: model,
            anchoredLines: anchoredLines
        )
        self.commentsByLine = commentsByLine
        self.composingLine = composingLine
        _commentDraft = commentDraft
        self.isSubmittingComment = isSubmittingComment
        self.showsCommentControls = true
        self.onRequestComment = onRequestComment
        self.onCancelComment = onCancelComment
        self.onSubmitComment = onSubmitComment
        self.onReply = onReply
    }

    /// Read-only presentation without review comment plumbing.
    init(presentation: UnifiedDiffPresentation) {
        self.presentation = presentation
        self.commentsByLine = [:]
        self.composingLine = nil
        _commentDraft = .constant("")
        self.isSubmittingComment = false
        self.showsCommentControls = false
        self.onRequestComment = { _ in }
        self.onCancelComment = {}
        self.onSubmitComment = { _ in }
        self.onReply = { _ in }
    }

    var body: some View {
        Group {
            if presentation.changedLineCount == 0 {
                Text("No content changes")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 96)
            } else {
                ScrollView(.horizontal, showsIndicators: true) {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(presentation.blocks) { block in
                            blockView(block)
                        }
                    }
                    .frame(minWidth: minimumContentWidth, alignment: .leading)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .textBackgroundColor))
        .background {
            GeometryReader { proxy in
                Color.clear.preference(
                    key: UnifiedDiffViewportWidthKey.self,
                    value: proxy.size.width
                )
            }
        }
        .onPreferenceChange(UnifiedDiffViewportWidthKey.self) { viewportWidth = $0 }
    }

    private var minimumContentWidth: CGFloat {
        max(viewportWidth, 1)
    }

    private var lineGutterWidth: CGFloat {
        UnifiedDiffMetrics.lineGutterWidth(
            maximumLineNumber: presentation.maximumLineNumber
        )
    }

    private var contentLeadingInset: CGFloat {
        UnifiedDiffMetrics.contentLeadingInset(
            lineGutterWidth: lineGutterWidth,
            showsRemoteLineNumbers: presentation.showsRemoteLineNumbers
        )
    }

    private var inlineCommentWidth: CGFloat {
        UnifiedDiffMetrics.inlineCommentWidth(
            viewportWidth: minimumContentWidth,
            leadingInset: contentLeadingInset
        )
    }

    @ViewBuilder
    private func blockView(_ block: UnifiedDiffBlockPresentation) -> some View {
        switch block.kind {
        case .hunk(let label):
            hunkHeader(label)
            ForEach(block.lines) { line in
                lineView(line)
            }
        case .omission:
            omissionControl(block)
            if omissionIsExpanded(block) {
                ForEach(block.lines) { line in
                    lineView(line)
                }
            }
        }
    }

    private func hunkHeader(_ label: String) -> some View {
        Text(label)
            .font(.caption.monospaced())
            .foregroundStyle(.secondary)
            .padding(.horizontal, 10)
            .frame(
                minWidth: minimumContentWidth,
                maxWidth: .infinity,
                minHeight: 26,
                alignment: .leading
            )
            .background(Color.accentColor.opacity(0.06))
    }

    private func omissionControl(_ block: UnifiedDiffBlockPresentation) -> some View {
        Button {
            if expandedOmissionIds.contains(block.id) {
                expandedOmissionIds.remove(block.id)
            } else {
                expandedOmissionIds.insert(block.id)
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: omissionIsExpanded(block) ? "chevron.up" : "chevron.down")
                Text("\(block.lines.count) unchanged lines")
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 10)
            .frame(
                minWidth: minimumContentWidth,
                maxWidth: .infinity,
                minHeight: 26,
                alignment: .leading
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .focusEffectDisabled()
        .accessibilityLabel(
            omissionIsExpanded(block)
                ? "Collapse \(block.lines.count) unchanged lines"
                : "Expand \(block.lines.count) unchanged lines"
        )
    }

    private func omissionIsExpanded(_ block: UnifiedDiffBlockPresentation) -> Bool {
        if expandedOmissionIds.contains(block.id) {
            return true
        }
        return block.lines.contains { line in
            guard let anchor = line.commentAnchorLine else { return false }
            return commentsByLine[anchor] != nil || composingLine == anchor
        }
    }

    @ViewBuilder
    private func lineView(_ line: UnifiedDiffLine) -> some View {
        diffRow(line)
        if let anchor = line.commentAnchorLine,
           let thread = commentsByLine[anchor] {
            commentThread(thread, line: anchor)
        }
        if composingLine == line.commentAnchorLine,
           let anchor = line.commentAnchorLine {
            composer(for: anchor)
        }
    }

    private func diffRow(_ line: UnifiedDiffLine) -> some View {
        HStack(alignment: .top, spacing: 0) {
            lineNumber(line.oldLineNumber, width: lineGutterWidth)
            lineNumber(line.newLineNumber, width: lineGutterWidth)
            if presentation.showsRemoteLineNumbers {
                lineNumber(line.remoteLineNumber, width: lineGutterWidth)
            }

            Text(marker(for: line.kind))
                .foregroundStyle(markerColor(for: line.kind))
                .frame(width: UnifiedDiffMetrics.markerWidth)

            commentControl(for: line)

            Text(line.text.isEmpty ? " " : line.text)
                .foregroundStyle(.primary)
                .textSelection(.enabled)
                .fixedSize(horizontal: true, vertical: false)
                .padding(.trailing, 12)
        }
        .fixedSize(horizontal: false, vertical: true)
        .font(.system(size: 12, design: .monospaced))
        .padding(.vertical, 3)
        .frame(
            minWidth: minimumContentWidth,
            maxWidth: .infinity,
            minHeight: 24,
            alignment: .topLeading
        )
        .background(rowBackground(for: line.kind))
        .contentShape(Rectangle())
        .onHover { hovering in
            hoveredLineId = hovering ? line.id : (hoveredLineId == line.id ? nil : hoveredLineId)
        }
    }

    private func lineNumber(_ value: Int?, width: CGFloat) -> some View {
        Text(value.map(String.init) ?? "")
            .foregroundStyle(.tertiary)
            .padding(.trailing, 8)
            .frame(width: width, alignment: .trailing)
            .frame(maxHeight: .infinity, alignment: .topTrailing)
            .background(Color(nsColor: .controlBackgroundColor).opacity(0.48))
    }

    @ViewBuilder
    private func commentControl(for line: UnifiedDiffLine) -> some View {
        if showsCommentControls, let anchor = line.commentAnchorLine {
            Button {
                onRequestComment(anchor)
            } label: {
                Image(systemName: "plus")
            }
            .buttonStyle(.borderless)
            .foregroundStyle(.secondary)
            .opacity(hoveredLineId == line.id ? 1 : 0)
            .frame(width: UnifiedDiffMetrics.commentButtonWidth)
            .help("Comment on new line \(anchor)")
            .accessibilityLabel("Comment on new line \(anchor)")
        } else {
            Color.clear
                .frame(width: UnifiedDiffMetrics.commentButtonWidth, height: 1)
                .accessibilityHidden(true)
        }
    }

    private func commentThread(_ comments: [ReviewComment], line: Int) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(comments) { comment in
                ReviewCommentRow(comment: comment) {
                    onReply(line)
                }
            }
        }
        .frame(width: inlineCommentWidth, alignment: .leading)
        .padding(.leading, contentLeadingInset)
        .padding(.trailing, 12)
        .padding(.vertical, 8)
        .frame(width: minimumContentWidth, alignment: .leading)
        .background(Color.accentColor.opacity(0.035))
    }

    private func composer(for line: Int) -> some View {
        ReviewCommentComposer(
            text: $commentDraft,
            isSubmitting: isSubmittingComment,
            onCancel: onCancelComment,
            onSubmit: { onSubmitComment(line) }
        )
        .frame(width: inlineCommentWidth, alignment: .leading)
        .padding(.leading, contentLeadingInset)
        .padding(.trailing, 12)
        .padding(.vertical, 8)
        .frame(width: minimumContentWidth, alignment: .leading)
        .background(Color.accentColor.opacity(0.035))
    }

    private func marker(for kind: UnifiedDiffLineKind) -> String {
        switch kind {
        case .context: " "
        case .insertion, .remoteInsertion: "+"
        case .removal, .remoteRemoval: "−"
        }
    }

    private func markerColor(for kind: UnifiedDiffLineKind) -> Color {
        switch kind {
        case .context: .secondary
        case .insertion: .green
        case .removal: .red
        case .remoteInsertion, .remoteRemoval: .gray
        }
    }

    private func rowBackground(for kind: UnifiedDiffLineKind) -> Color {
        switch kind {
        case .context: .clear
        case .insertion: .green.opacity(0.08)
        case .removal: .red.opacity(0.08)
        case .remoteInsertion, .remoteRemoval: .gray.opacity(0.08)
        }
    }
}
