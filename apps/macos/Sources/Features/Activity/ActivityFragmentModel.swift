import Combine
import Foundation

@MainActor
final class ActivityFragmentModel: ObservableObject {
    @Published private(set) var fullFragment: RecallFragment?
    @Published private(set) var isLoading = false
    @Published private(set) var loadFailed = false
    private var loadGeneration = UUID()
    private var loadedRunId: String?

    func load(fragment: RecallFragment, runId: String?,
              fetch: () async throws -> RecallFragment) async {
        if loadedRunId != runId {
            loadGeneration = UUID()
            isLoading = false
            loadFailed = false
            fullFragment = nil
            loadedRunId = runId
        }
        guard fullFragment == nil, runId != nil,
              fragment.truncated || fragment.content.isEmpty else { return }
        let generation = UUID()
        loadGeneration = generation
        isLoading = true
        loadFailed = false
        defer { if loadGeneration == generation { isLoading = false } }
        do {
            let loaded = try await fetch()
            try Task.checkCancellation()
            guard loadGeneration == generation else { return }
            fullFragment = loaded
        } catch is CancellationError {
        } catch {
            if loadGeneration == generation { loadFailed = true }
        }
    }
}
