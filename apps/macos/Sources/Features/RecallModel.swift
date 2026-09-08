import Foundation

struct RecallRetrievalSelection: Equatable {
    let sessionId: String
    let sessionTitle: String
    let requestNumber: Int
    let requestText: String
    let runId: String
}

@MainActor
final class RecallModel: ObservableObject {
    @Published private(set) var sessions: [RecallSession] = []
    @Published var selectedSessionId: String? {
        didSet {
            if selectedSessionId != oldValue { retrievalSelection = nil }
        }
    }
    @Published private(set) var retrievalSelection: RecallRetrievalSelection?
    @Published private(set) var selectedProjectId: String?
    @Published private(set) var isLoading = false
    @Published var errorMessage: String?

    private let daemon: DaemonXPCClient

    init(daemon: DaemonXPCClient) {
        self.daemon = daemon
    }

    func load() async {
        let projectId = selectedProjectId
        isLoading = true
        errorMessage = nil
        defer { isLoading = false }
        do {
            let response = try await daemon.listRecalls(
                ListRecallsRequest(projectId: projectId)
            )
            guard projectId == selectedProjectId else { return }
            sessions = response.sessions
            if selectedSessionId == nil || !sessions.contains(where: { $0.id == selectedSessionId }) {
                selectedSessionId = sessions.first?.id
            }
        } catch {
            guard projectId == selectedProjectId else { return }
            errorMessage = error.localizedDescription
        }
    }

    func selectProject(_ projectId: String?) async {
        guard projectId != selectedProjectId else { return }
        retrievalSelection = nil
        selectedProjectId = projectId
        await load()
    }

    var selectedSession: RecallSession? {
        sessions.first { $0.id == selectedSessionId }
    }

    func openRetrieval(
        session: RecallSession,
        task: RecallTask,
        requestNumber: Int,
        activation: RecallActivation
    ) {
        guard session.id == selectedSessionId, let runId = activation.runId else { return }
        retrievalSelection = RecallRetrievalSelection(
            sessionId: session.id,
            sessionTitle: session.activityDisplayTitle,
            requestNumber: requestNumber,
            requestText: task.text,
            runId: runId
        )
    }

    func closeRetrieval() {
        retrievalSelection = nil
    }

    func loadFragment(
        workspaceRoot: String,
        runId: String,
        unitKey: String
    ) async throws -> RecallFragment {
        try await daemon.recallFragment(
            GetRecallFragmentRequest(
                workspaceRoot: workspaceRoot,
                runId: runId,
                unitKey: unitKey
            )
        ).fragment
    }
}
