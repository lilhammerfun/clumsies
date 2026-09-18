import Foundation

struct EvaluationEvidenceDraft: Identifiable, Equatable, Sendable {
    var id: String {
        "\(resourceId)\u{0}\(unitKey ?? "")"
    }

    let resourceId: String
    let unitKey: String?

    init(resourceId: String, unitKey: String?) {
        self.resourceId = resourceId
        self.unitKey = unitKey
    }

    init(_ evidence: EvaluationEvidence) {
        self.init(
            resourceId: evidence.resourceId,
            unitKey: evidence.unitKey
        )
    }

    var input: EvaluationEvidenceInput {
        EvaluationEvidenceInput(
            resourceId: resourceId,
            unitKey: unitKey
        )
    }
}

@MainActor
final class RetrievalDiagnosticsModel: ObservableObject {
    @Published private(set) var runs: [RetrievalRun] = []
    @Published private(set) var selectedRunId: String?
    @Published private(set) var detail: RetrievalRunDetail?
    @Published private(set) var evidenceDrafts: [EvaluationEvidenceDraft] = []
    @Published private(set) var isLoading = false
    @Published private(set) var isLoadingMore = false
    @Published private(set) var isMutating = false
    @Published var errorMessage: String?

    private let daemon: DaemonXPCClient
    private let fetchRuns: @MainActor (RetrievalRunListRequest) async throws -> RetrievalRunListResponse
    private var listGeneration = UUID()
    private let fetchRun: @MainActor (String) async throws -> RetrievalRunDetail
    private var projectId: String?
    private(set) var nextCursor: String?
    private var selectionGeneration = UUID()

    init(
        daemon: DaemonXPCClient,
        fetchRuns: (@MainActor (RetrievalRunListRequest) async throws -> RetrievalRunListResponse)? = nil,
        fetchRun: (@MainActor (String) async throws -> RetrievalRunDetail)? = nil
    ) {
        self.daemon = daemon
        self.fetchRuns = fetchRuns ?? { try await daemon.listRetrievalRuns($0) }
        self.fetchRun = fetchRun ?? { try await daemon.retrievalRun($0) }
    }

    func load(projectId: String?) async {
        if self.projectId != projectId {
            runs = []
            nextCursor = nil
            selectedRunId = nil
            detail = nil
            evidenceDrafts = []
        }
        self.projectId = projectId
        let generation = UUID()
        selectionGeneration = generation
        listGeneration = generation
        isLoadingMore = false
        isLoading = true
        errorMessage = nil
        defer {
            if selectionGeneration == generation {
                isLoading = false
            }
        }
        do {
            let response = try await fetchRuns(
                RetrievalRunListRequest(
                    projectId: projectId,
                    status: nil,
                    cursor: nil,
                    limit: 100
                )
            )
            guard selectionGeneration == generation, !Task.isCancelled else { return }
            runs = response.items
            nextCursor = response.nextCursor
            let selected = selectedRunId.flatMap { selected in
                response.items.first(where: { $0.runId == selected })?.runId
            } ?? response.items.first?.runId
            selectedRunId = selected
            if let selected {
                try await loadDetail(runId: selected, generation: generation)
            } else {
                detail = nil
                evidenceDrafts = []
            }
        } catch {
            guard selectionGeneration == generation, !Task.isCancelled else { return }
            runs = []
            nextCursor = nil
            detail = nil
            evidenceDrafts = []
            errorMessage = error.localizedDescription
        }
    }

    func loadMore() async {
        guard let cursor = nextCursor, !isLoadingMore, !isLoading else { return }
        let generation = listGeneration
        isLoadingMore = true
        errorMessage = nil
        defer { if listGeneration == generation { isLoadingMore = false } }
        do {
            let response = try await fetchRuns(
                RetrievalRunListRequest(
                    projectId: projectId,
                    status: nil,
                    cursor: cursor,
                    limit: 100
                )
            )
            try Task.checkCancellation()
            guard listGeneration == generation else { return }
            let existing = Set(runs.map(\.runId))
            runs.append(contentsOf: response.items.filter { !existing.contains($0.runId) })
            nextCursor = response.nextCursor
        } catch {
            guard listGeneration == generation, !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }

    func select(runId: String?) async {
        if let runId, runId == selectedRunId, detail?.run.runId == runId { return }
        selectedRunId = runId
        let generation = UUID()
        selectionGeneration = generation
        detail = nil
        evidenceDrafts = []
        errorMessage = nil
        isLoading = false
        guard let runId else { return }
        isLoading = true
        defer {
            if selectionGeneration == generation {
                isLoading = false
            }
        }
        do {
            try await loadDetail(runId: runId, generation: generation)
        } catch {
            guard selectionGeneration == generation, !Task.isCancelled else { return }
            detail = nil
            evidenceDrafts = []
            errorMessage = error.localizedDescription
        }
    }

    @discardableResult
    func markInaccurate() async -> Bool {
        guard let runId = detail?.run.runId else { return false }
        let generation = selectionGeneration
        return await mutate {
            _ = try await daemon.createEvaluationCase(
                CreateEvaluationCaseRequest(runId: runId)
            )
            guard selectionGeneration == generation, !Task.isCancelled else { return }
            try await loadDetail(runId: runId, generation: generation)
        }
    }

    @discardableResult
    func resolveEvidenceReview() async -> Bool {
        guard let runId = detail?.run.runId,
              let evaluationCase = detail?.evaluationCase else {
            return false
        }
        let evidence = evidenceDrafts.map(\.input)
        let generation = selectionGeneration
        return await mutate {
            _ = try await daemon.resolveEvaluationCase(
                ResolveEvaluationCaseRequest(
                    caseId: evaluationCase.caseId,
                    expectedVersion: evaluationCase.version,
                    evidence: evidence,
                    noneMatched: evidence.isEmpty
                )
            )
            guard selectionGeneration == generation, !Task.isCancelled else { return }
            try await loadDetail(runId: runId, generation: generation)
        }
    }

    func resetEvidenceSelection() {
        evidenceDrafts = detail?.evidence.map(EvaluationEvidenceDraft.init) ?? []
    }

    func isEvidenceSelected(_ suggestion: EvaluationEvidenceSuggestion) -> Bool {
        evidenceDrafts.contains {
            $0.resourceId == suggestion.resourceId && $0.unitKey == suggestion.unitKey
        }
    }

    func setEvidenceSelected(_ selected: Bool, suggestion: EvaluationEvidenceSuggestion) {
        evidenceDrafts.removeAll {
            $0.resourceId == suggestion.resourceId && $0.unitKey == suggestion.unitKey
        }
        if selected {
            evidenceDrafts.append(
                EvaluationEvidenceDraft(
                    resourceId: suggestion.resourceId,
                    unitKey: suggestion.unitKey
                )
            )
        }
    }

    func clearUnpinnedHistory() async {
        await mutate {
            _ = try await daemon.clearRetrievalRuns(projectId: projectId)
            await load(projectId: projectId)
        }
    }

    func exportEvaluationSet() async throws -> ExportEvaluationSetResponse {
        try await daemon.exportEvaluationSet(
            projectId: runs.isEmpty ? detail?.run.projectId : projectId
        )
    }

    private func loadDetail(runId: String, generation: UUID) async throws {
        let loaded = try await fetchRun(runId)
        guard selectionGeneration == generation,
              selectedRunId == loaded.run.runId,
              !Task.isCancelled else { return }
        detail = loaded
        evidenceDrafts = loaded.evidence.map(EvaluationEvidenceDraft.init)
        if let index = runs.firstIndex(where: { $0.runId == loaded.run.runId }) {
            runs[index] = loaded.run
        }
    }

    @discardableResult
    private func mutate(_ operation: () async throws -> Void) async -> Bool {
        guard !isMutating else { return false }
        isMutating = true
        let generation = selectionGeneration
        errorMessage = nil
        defer { isMutating = false }
        do {
            try await operation()
            return true
        } catch {
            if selectionGeneration == generation {
                errorMessage = error.localizedDescription
            }
            return false
        }
    }
}
