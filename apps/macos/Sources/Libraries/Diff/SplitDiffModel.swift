enum SplitDiffCellKind: Equatable, Sendable {
    case context
    case insertion
    case removal
}
struct SplitDiffCell: Equatable, Sendable {
    let kind: SplitDiffCellKind
    let text: String
    let lineNumber: Int
}

struct SplitDiffRow: Identifiable, Equatable, Sendable {
    let id: Int
    let original: SplitDiffCell?
    let modified: SplitDiffCell?

    var isChanged: Bool {
        original?.kind != .context || modified?.kind != .context
    }
}

enum SplitDiffBlockKind: Equatable, Sendable {
    case hunk(String)
    case omission
}

struct SplitDiffBlock: Identifiable, Equatable, Sendable {
    let id: Int
    let kind: SplitDiffBlockKind
    let rows: [SplitDiffRow]
}

struct SplitDiffModel: Equatable, Sendable {
    let rows: [SplitDiffRow]
    let blocks: [SplitDiffBlock]

    static func make(
        original: String,
        modified: String,
        contextLineCount: Int = 3
    ) -> Self {
        let rows = alignedRows(original: original, modified: modified)
        let changedIndices = rows.indices.filter { rows[$0].isChanged }
        guard !changedIndices.isEmpty else {
            return .init(rows: rows, blocks: [])
        }

        let context = max(0, contextLineCount)
        var ranges: [ClosedRange<Int>] = []
        for index in changedIndices {
            let candidate = max(0, index - context)...min(rows.count - 1, index + context)
            if let previous = ranges.last, candidate.lowerBound <= previous.upperBound + 1 {
                ranges[ranges.count - 1] = previous.lowerBound...max(
                    previous.upperBound,
                    candidate.upperBound
                )
            } else {
                ranges.append(candidate)
            }
        }

        var blocks: [SplitDiffBlock] = []
        var cursor = 0
        for range in ranges {
            if cursor < range.lowerBound {
                blocks.append(.init(
                    id: blocks.count,
                    kind: .omission,
                    rows: Array(rows[cursor..<range.lowerBound])
                ))
            }

            let hunkRows = Array(rows[range])
            blocks.append(.init(
                id: blocks.count,
                kind: .hunk(hunkHeader(for: range, in: rows)),
                rows: hunkRows
            ))
            cursor = range.upperBound + 1
        }

        if cursor < rows.count {
            blocks.append(.init(
                id: blocks.count,
                kind: .omission,
                rows: Array(rows[cursor...])
            ))
        }

        return .init(rows: rows, blocks: blocks)
    }

    private enum LineChange {
        case context(SplitDiffCell, SplitDiffCell)
        case removal(SplitDiffCell)
        case insertion(SplitDiffCell)
    }

    private static func alignedRows(original: String, modified: String) -> [SplitDiffRow] {
        let changes = lineChanges(original: original, modified: modified)
        var rows: [SplitDiffRow] = []
        var index = 0

        while index < changes.count {
            switch changes[index] {
            case .context(let original, let modified):
                rows.append(.init(
                    id: rows.count,
                    original: original,
                    modified: modified
                ))
                index += 1
            case .removal, .insertion:
                var removals: [SplitDiffCell] = []
                var insertions: [SplitDiffCell] = []
                while index < changes.count {
                    switch changes[index] {
                    case .context:
                        break
                    case .removal(let cell):
                        removals.append(cell)
                        index += 1
                        continue
                    case .insertion(let cell):
                        insertions.append(cell)
                        index += 1
                        continue
                    }
                    break
                }

                for offset in 0..<max(removals.count, insertions.count) {
                    rows.append(.init(
                        id: rows.count,
                        original: removals.indices.contains(offset) ? removals[offset] : nil,
                        modified: insertions.indices.contains(offset) ? insertions[offset] : nil
                    ))
                }
            }
        }

        return rows
    }

    private static func lineChanges(original: String, modified: String) -> [LineChange] {
        let oldLines = lines(in: original)
        let newLines = lines(in: modified)
        let difference = newLines.difference(from: oldLines)
        var removals: [Int: String] = [:]
        var insertions: [Int: String] = [:]

        for change in difference {
            switch change {
            case .remove(let offset, let element, _):
                removals[offset] = element
            case .insert(let offset, let element, _):
                insertions[offset] = element
            }
        }

        var changes: [LineChange] = []
        var oldIndex = 0
        var newIndex = 0
        while oldIndex < oldLines.count || newIndex < newLines.count {
            if let value = removals[oldIndex] {
                changes.append(.removal(.init(
                    kind: .removal,
                    text: value,
                    lineNumber: oldIndex + 1
                )))
                oldIndex += 1
                continue
            }
            if let value = insertions[newIndex] {
                changes.append(.insertion(.init(
                    kind: .insertion,
                    text: value,
                    lineNumber: newIndex + 1
                )))
                newIndex += 1
                continue
            }
            if oldIndex < oldLines.count, newIndex < newLines.count {
                if oldLines[oldIndex] == newLines[newIndex] {
                    changes.append(.context(
                        .init(
                            kind: .context,
                            text: oldLines[oldIndex],
                            lineNumber: oldIndex + 1
                        ),
                        .init(
                            kind: .context,
                            text: newLines[newIndex],
                            lineNumber: newIndex + 1
                        )
                    ))
                } else {
                    changes.append(.removal(.init(
                        kind: .removal,
                        text: oldLines[oldIndex],
                        lineNumber: oldIndex + 1
                    )))
                    changes.append(.insertion(.init(
                        kind: .insertion,
                        text: newLines[newIndex],
                        lineNumber: newIndex + 1
                    )))
                }
                oldIndex += 1
                newIndex += 1
            } else if oldIndex < oldLines.count {
                changes.append(.removal(.init(
                    kind: .removal,
                    text: oldLines[oldIndex],
                    lineNumber: oldIndex + 1
                )))
                oldIndex += 1
            } else {
                changes.append(.insertion(.init(
                    kind: .insertion,
                    text: newLines[newIndex],
                    lineNumber: newIndex + 1
                )))
                newIndex += 1
            }
        }
        return changes
    }

    private static func hunkHeader(
        for range: ClosedRange<Int>,
        in rows: [SplitDiffRow]
    ) -> String {
        let hunkRows = rows[range]
        let oldNumbers = hunkRows.compactMap(\.original?.lineNumber)
        let newNumbers = hunkRows.compactMap(\.modified?.lineNumber)
        let old = lineRange(numbers: oldNumbers, before: rows[..<range.lowerBound].last?.original)
        let new = lineRange(numbers: newNumbers, before: rows[..<range.lowerBound].last?.modified)
        return "@@ -\(old.start),\(old.count) +\(new.start),\(new.count) @@"
    }

    private static func lineRange(
        numbers: [Int],
        before: SplitDiffCell?
    ) -> (start: Int, count: Int) {
        if let first = numbers.first {
            return (first, numbers.count)
        }
        return (before?.lineNumber ?? 0, 0)
    }

    private static func lines(in text: String) -> [String] {
        guard !text.isEmpty else { return [] }
        return text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    }
}
