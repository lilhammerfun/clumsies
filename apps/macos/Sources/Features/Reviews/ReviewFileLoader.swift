import Foundation

struct ReviewFileContent: Sendable {
    let sources: ReviewChangeSources
    let diff: SplitDiffModel?
}

/// Shares immutable snapshots only within one loaded Review revision.
actor ReviewFileLoader {
    private let fetch: @Sendable (String) async throws -> CommitPayload
    private var commits: [String: Task<CommitPayload, Error>] = [:]
    private var cancelled = false

    init(fetch: @escaping @Sendable (String) async throws -> CommitPayload) {
        self.fetch = fetch
    }

    func load(_ detail: ReviewDraftDetail) async throws -> ReviewFileContent {
        try checkCancellation()
        async let base = commit(detail.draft.baseCommitId)
        async let current = commit(detail.draft.coordination.currentCommitId)
        let sources = try await Self.mapReviewChangeSources(
            draft: detail.draft, operations: detail.operations, base: base, current: current
        )
        try checkCancellation()
        let diff = sources.draftContent.map {
            SplitDiffModel.make(original: sources.baseContent ?? "", modified: $0)
        }
        try checkCancellation()
        return ReviewFileContent(sources: sources, diff: diff)
    }

    func cancel() {
        cancelled = true
        for task in commits.values { task.cancel() }
        commits.removeAll()
    }

    private func commit(_ id: String?) async throws -> CommitPayload? {
        try checkCancellation()
        guard let id else { return nil }
        if let task = commits[id] { return try await task.value }
        let task = Task { try await fetch(id) }
        commits[id] = task
        do {
            return try await task.value
        } catch {
            commits[id] = nil
            throw error
        }
    }

    nonisolated static func mapReviewChangeSources(
        detail: ReviewDetail,
        base: CommitPayload?,
        current: CommitPayload?
    ) throws -> ReviewChangeSources {
        try mapReviewChangeSources(
            draft: detail.draft,
            operations: detail.operations,
            base: base,
            current: current
        )
    }

    nonisolated static func mapReviewChangeSources(
        draft: ServerDraft,
        operations: [ServerDraftOperation],
        base: CommitPayload?,
        current: CommitPayload?
    ) throws -> ReviewChangeSources {
        let baseEntry = commitResourceEntry(base, resource: draft.resource)
        let currentEntry = commitResourceEntry(current, resource: draft.resource)
        let terminalOperation = operations.last
        let draftContent: String?
        let resolutionContent: String?
        if terminalOperation?.action == "delete" {
            draftContent = nil
            resolutionContent = nil
        } else {
            let content = operations.reversed().first {
                ($0.action == "create" || $0.action == "update") && $0.content != nil
            }?.content
            draftContent = content?.renderedText
            resolutionContent = content?.primaryText
        }
        let initialPath = operations.first?.resource.path
            ?? draft.resource.path
            ?? baseEntry?.path
            ?? currentEntry?.path
        let finalPath = operations.reduce(initialPath) { path, operation in
            if let newPath = operation.newPath { return newPath }
            if operation.action == "create", let createdPath = operation.resource.path { return createdPath }
            return path
        }
        let operationLabels: [String]
        if terminalOperation?.action == "delete" {
            operationLabels = [String(localized: "Delete \(finalPath ?? String(localized: "the selected memory"))")]
        } else if operations.first?.action == "create" {
            operationLabels = [String(localized: "Create \(finalPath ?? "memory")")]
        } else if finalPath != initialPath, let finalPath {
            operationLabels = [String(localized: "Rename to \(finalPath)")]
        } else {
            operationLabels = []
        }
        return try .init(
            baseContent: commitResourceText(base, entry: baseEntry),
            currentContent: commitResourceText(current, entry: currentEntry),
            draftContent: draftContent,
            resolutionContent: resolutionContent,
            proposedPath: finalPath,
            operationLabels: operationLabels
        )
    }

    private nonisolated static func commitResourceEntry(
        _ payload: CommitPayload?,
        resource: ServerDraftResourceReference
    ) -> CommitTreeEntry? {
        payload?.tree.entries.first { candidate in
            guard candidate.type == .memory else { return false }
            if let resourceId = resource.id { return candidate.id == resourceId }
            return candidate.path == resource.path
        }
    }

    private nonisolated static func commitResourceText(
        _ payload: CommitPayload?,
        entry: CommitTreeEntry?
    ) throws -> String? {
        guard let payload,
              let entry,
              let blob = payload.blobs.first(where: { $0.blobId == entry.blobId }) else { return nil }
        return blob.content
    }

    private func checkCancellation() throws {
        try Task.checkCancellation()
        if cancelled { throw CancellationError() }
    }
}
