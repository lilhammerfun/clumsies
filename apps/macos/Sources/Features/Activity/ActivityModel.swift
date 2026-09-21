import Foundation

struct ActivityRetrievalSelection: Hashable {
    let sessionId: String
    let runId: String
}

@MainActor
final class ActivityModel: ObservableObject {
    @Published private(set) var sessions: [RecallSessionSummary] = []
    @Published var selectedSessionId: String? {
        didSet {
            if selectedSessionId != oldValue { resetDetail() }
        }
    }
    @Published private(set) var selectedSession: RecallSession?
    @Published private(set) var totalTasks = 0
    @Published private(set) var retrievalSelection: ActivityRetrievalSelection?
    @Published private(set) var selectedProjectId: String?
    @Published private(set) var isLoading = false
    @Published private(set) var isLoadingMore = false
    @Published private(set) var isLoadingDetail = false
    @Published private(set) var hasLoaded = false
    @Published private(set) var nextCursor: String?
    @Published private(set) var nextTaskOffset: Int?
    @Published private(set) var errorMessage: String?
    @Published private(set) var pageError: String?
    @Published private(set) var detailError: String?

    private let daemon: DaemonXPCClient
    private let defaults: UserDefaults
    private let fetchSessions: @MainActor (ListRecallsRequest) async throws -> ListRecallsResponse
    private let fetchDetail: @MainActor (GetRecallSessionRequest) async throws -> GetRecallSessionResponse
    private var loadGeneration = UUID()
    private var detailGeneration = UUID()
    private var failedTaskOffset: Int?
    private var preferenceKey: String?
    private var prepared = false
    private var explicitlyAllProjects = false

    init(
        daemon: DaemonXPCClient,
        defaults: UserDefaults = .standard,
        fetchSessions: (@MainActor (ListRecallsRequest) async throws -> ListRecallsResponse)? = nil,
        fetchDetail: (@MainActor (GetRecallSessionRequest) async throws -> GetRecallSessionResponse)? = nil
    ) {
        self.daemon = daemon
        self.defaults = defaults
        self.fetchSessions = fetchSessions ?? { try await daemon.listRecalls($0) }
        self.fetchDetail = fetchDetail ?? { try await daemon.recallSession($0) }
    }

    /// Restore before issuing any list request. All Projects remains an explicit
    /// in-session choice; reopening Activity prefers the last concrete project.
    func prepare(projectIds: [String], preferredProjectId: String?, scope: String) {
        let key = "ClumsiesActivityProject.\(scope)"
        let scopeChanged = preferenceKey != key
        if prepared && !scopeChanged {
            if let selectedProjectId, projectIds.contains(selectedProjectId) { return }
            if selectedProjectId == nil && explicitlyAllProjects { return }
        }
        preferenceKey = key
        prepared = true
        let saved = defaults.string(forKey: key)
        let project = [saved, preferredProjectId].compactMap { $0 }.first { projectIds.contains($0) }
            ?? projectIds.first
        explicitlyAllProjects = false
        resetList(projectId: project)
        hasLoaded = projectIds.isEmpty
        if let project { defaults.set(project, forKey: key) }
    }

    var selectedSummary: RecallSessionSummary? {
        sessions.first { $0.id == selectedSessionId }
    }

    func load() async {
        guard !isLoading else { return }
        let generation = UUID()
        loadGeneration = generation
        isLoading = true
        isLoadingMore = false
        errorMessage = nil
        pageError = nil
        let projectId = selectedProjectId
        let retainedCount = sessions.count
        defer { if loadGeneration == generation { isLoading = false } }
        do {
            var response = try await fetchSessions(.init(projectId: projectId))
            var refreshed = response.sessions
            // Refresh enough summaries to keep an already-scrolled list intact.
            while refreshed.count < retainedCount, let cursor = response.nextCursor {
                try Task.checkCancellation()
                guard loadGeneration == generation else { return }
                response = try await fetchSessions(.init(projectId: projectId, cursor: cursor))
                refreshed.append(contentsOf: response.sessions)
            }
            try Task.checkCancellation()
            guard loadGeneration == generation else { return }
            sessions = refreshed
            nextCursor = response.nextCursor
            hasLoaded = true
            if selectedSessionId == nil || !sessions.contains(where: { $0.id == selectedSessionId }) {
                selectedSessionId = sessions.first?.id
            }
        } catch is CancellationError {
            return
        } catch {
            guard loadGeneration == generation, !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }

    func loadMoreSessions() async {
        guard !isLoading, !isLoadingMore, let cursor = nextCursor else { return }
        let generation = loadGeneration
        isLoadingMore = true
        pageError = nil
        defer { if loadGeneration == generation { isLoadingMore = false } }
        do {
            let response = try await fetchSessions(.init(projectId: selectedProjectId, cursor: cursor))
            try Task.checkCancellation()
            guard generation == loadGeneration else { return }
            let known = Set(sessions.map(\.id))
            sessions.append(contentsOf: response.sessions.filter { !known.contains($0.id) })
            nextCursor = response.nextCursor
        } catch is CancellationError {
            return
        } catch {
            guard generation == loadGeneration, !Task.isCancelled else { return }
            pageError = error.localizedDescription
        }
    }

    func selectProject(_ projectId: String?) async {
        guard projectId != selectedProjectId else { return }
        explicitlyAllProjects = projectId == nil
        resetList(projectId: projectId)
        if let projectId, let preferenceKey { defaults.set(projectId, forKey: preferenceKey) }
        await load()
    }

    private func resetList(projectId: String?) {
        loadGeneration = UUID()
        selectedProjectId = projectId
        selectedSessionId = nil
        resetDetail()
        sessions = []
        nextCursor = nil
        hasLoaded = false
        isLoading = false
        isLoadingMore = false
        errorMessage = nil
        pageError = nil
    }

    private func resetDetail() {
        detailGeneration = UUID()
        retrievalSelection = nil
        selectedSession = nil
        totalTasks = 0
        nextTaskOffset = nil
        isLoadingDetail = false
        detailError = nil
        failedTaskOffset = nil
    }

    func retryDetail() async {
        if failedTaskOffset != nil { await loadMoreTasks() }
        else { await loadSelectedSession() }
    }

    func loadSelectedSession() async {
        guard let summary = selectedSummary else { return }
        detailGeneration = UUID()
        await loadTaskPage(summary: summary, offset: nil, generation: detailGeneration)
    }

    func loadMoreTasks() async {
        guard !isLoadingDetail, let summary = selectedSummary, let offset = nextTaskOffset else { return }
        await loadTaskPage(summary: summary, offset: offset, generation: detailGeneration)
    }

    private func loadTaskPage(summary: RecallSessionSummary, offset: Int?, generation: UUID) async {
        isLoadingDetail = true
        detailError = nil
        defer { if detailGeneration == generation { isLoadingDetail = false } }
        do {
            var response = try await fetchDetail(.init(sessionToken: summary.sessionToken, offset: offset))
            var loaded = response.session
            let retainedCount = offset == nil ? (selectedSession?.tasks.count ?? 0) : 0
            while loaded.tasks.count < retainedCount, let next = response.nextOffset {
                try Task.checkCancellation()
                guard detailGeneration == generation, selectedSummary?.sessionToken == summary.sessionToken else { return }
                response = try await fetchDetail(.init(sessionToken: summary.sessionToken, offset: next))
                loaded.tasks.append(contentsOf: response.session.tasks)
            }
            try Task.checkCancellation()
            guard detailGeneration == generation, selectedSummary?.sessionToken == summary.sessionToken else { return }
            if offset != nil {
                selectedSession?.tasks.append(contentsOf: response.session.tasks)
            } else {
                selectedSession = loaded
            }
            totalTasks = response.totalTasks
            nextTaskOffset = response.nextOffset
        } catch is CancellationError {
            return
        } catch {
            guard detailGeneration == generation, !Task.isCancelled else { return }
            failedTaskOffset = offset
            detailError = error.localizedDescription
        }
    }

    func openRetrieval(session: RecallSession, activation: RecallActivation) {
        guard session.id == selectedSessionId, let runId = activation.runId else { return }
        retrievalSelection = ActivityRetrievalSelection(
            sessionId: session.id, runId: runId
        )
    }

    func closeRetrieval() { retrievalSelection = nil }

    func loadFragment(workspaceRoot: String, runId: String, unitKey: String) async throws -> RecallFragment {
        try await daemon.recallFragment(
            GetRecallFragmentRequest(workspaceRoot: workspaceRoot, runId: runId, unitKey: unitKey)
        ).fragment
    }
}
