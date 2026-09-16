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
        let sources = try await WorkspaceLoader.mapReviewChangeSources(
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

    private func checkCancellation() throws {
        try Task.checkCancellation()
        if cancelled { throw CancellationError() }
    }
}
