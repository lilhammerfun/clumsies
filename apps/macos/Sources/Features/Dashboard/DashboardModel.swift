import Combine
import Foundation

@MainActor
final class DashboardModel: ObservableObject {
    @Published var period = 30
    @Published private(set) var snapshot: DashboardSnapshot?
    @Published private(set) var isDemo = false
    @Published private(set) var isLoading = false
    @Published private(set) var errorMessage: String?
    private var generation = UUID()
    private var contextKey: String?

    var summary: DashboardSummary? {
        snapshot.map { DashboardSummary(snapshot: $0) }
    }

    func load(key: String, fetch: () async throws -> (DashboardSnapshot, Bool)) async {
        if contextKey != key {
            snapshot = nil
            isDemo = false
        }
        contextKey = key
        let request = UUID()
        generation = request
        isLoading = true
        errorMessage = nil
        defer { if generation == request { isLoading = false } }
        do {
            let (value, demo) = try await fetch()
            try Task.checkCancellation()
            guard generation == request else { return }
            snapshot = value
            isDemo = demo
        } catch is CancellationError {
        } catch {
            guard generation == request, !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }

    /// Fixtures are opt-in files owned by an isolated Dev Instance, never the stable app.
    static func demoSnapshot(projectID: String?, period: Int) async throws -> DashboardSnapshot? {
        guard ClumsiesIdentifiers.developmentInstanceID != nil,
              let root = Bundle.main.object(forInfoDictionaryKey: "CLUMSIES_DAEMON_ROOT") as? String,
              !root.isEmpty else { return nil }
        let url = URL(fileURLWithPath: root).deletingLastPathComponent()
            .appending(path: "fixtures/dashboard.json")
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        return try await Task.detached(priority: .userInitiated) {
            let decoder = JSONCoding.decoder()
            let snapshots = try decoder.decode([DashboardSnapshot].self, from: Data(contentsOf: url))
            return snapshots.first { $0.projectID == projectID && $0.period == period }
        }.value
    }

    static func liveSnapshot(
        projectID: String?, name: String, period: Int, daemon: DaemonXPCClient, server: ServerClient
    ) async throws -> DashboardSnapshot {
        let prefix = projectID.map { "/api/v1/projects/\($0)" } ?? "/api/v1/org"
        let result: (value: DashboardMemoryStatistics, response: DaemonServerResponse) = try await server.getWithMetadata("\(prefix)/memory-statistics", query: [
            .init(name: "days", value: String(period)),
            .init(name: "time_zone", value: TimeZone.current.identifier)
        ])
        let memory = result.value
        try Task.checkCancellation()
        let retrieval = try await daemon.dashboardRetrievalStatistics(.init(
            projectIds: memory.projectIds, resources: memory.resources,
            dayBounds: memory.dayBounds, recencyStarts: memory.recencyStarts, generatedAt: memory.generatedAt
        ))
        return .init(projectId: projectID, projectName: name, period: period, memory: memory, retrieval: retrieval,
            notice: result.response.isStaleCache ? "Showing cached server statistics. Refresh when the server is available." : nil)
    }
}
