import Combine
import Foundation

@MainActor
final class ProjectMemberPickerModel: ObservableObject {
    private let administration: AdministrationModel
    private let fetchCandidates: (String, String?) async throws -> ListResponse<UserReference>
    private var authorityObservation: AnyCancellable?
    let projectId: String

    init(projectId: String, administration: AdministrationModel,
         fetchCandidates: ((String, String?) async throws -> ListResponse<UserReference>)? = nil) {
        self.projectId = projectId
        self.administration = administration
        self.fetchCandidates = fetchCandidates ?? { query, cursor in
            try await administration.searchProjectMemberCandidates(projectId: projectId, query: query, cursor: cursor)
        }
        authorityObservation = administration.$refreshGeneration.dropFirst().sink { [weak self] _ in
            self?.resetSearch()
        }
    }

    @Published private(set) var searchGeneration = UUID()
    @Published var query = "" {
        didSet { if query != oldValue { resetSearch() } }
    }
    @Published private(set) var members: [UserReference] = []
    @Published var selectedId: String?
    @Published var role: ProjectMemberRole = .member
    @Published private(set) var nextCursor: String?
    @Published private(set) var isLoading = true
    @Published private(set) var errorMessage: String?
    @Published private(set) var loadFailed = false
    private var loadMoreTask: Task<Void, Never>?

    var availableMembers: [UserReference] {
        let existingIds = Set((administration.projectMembers[projectId] ?? []).map(\.id))
        return members.filter { !existingIds.contains($0.id) }
    }

    var canAdd: Bool {
        let detail = administration.projectDetailStates[projectId]
        return administration.canMutateProject(projectId) && detail?.isStale != true && detail?.isLoading != true
            && !administration.loadingProjectIds.contains(projectId)
            && administration.projectMembers[projectId] != nil
            && role != .owner
            && availableMembers.contains { $0.id == self.selectedId }
    }

    func cancel() {
        searchGeneration = UUID()
        loadMoreTask?.cancel()
        loadMoreTask = nil
    }

    func search() async {
        do {
            try await Task.sleep(for: .milliseconds(200))
            await loadMembers()
        } catch {}
    }

    func loadMore() {
        guard !isLoading, let nextCursor else { return }
        loadMoreTask = Task { await self.loadMembers(cursor: nextCursor) }
    }

    func retry() {
        guard !isLoading else { return }
        loadMoreTask = Task { await self.loadMembers(cursor: self.members.isEmpty ? nil : self.nextCursor) }
    }

    private func resetSearch() {
        searchGeneration = UUID()
        loadMoreTask?.cancel()
        members = []
        selectedId = nil
        nextCursor = nil
        errorMessage = nil
        loadFailed = false
        isLoading = true
    }

    func loadMembers(cursor: String? = nil) async {
        let generation = searchGeneration
        let requestedQuery = query
        isLoading = true
        errorMessage = nil
        loadFailed = false
        defer { if generation == searchGeneration && !Task.isCancelled { isLoading = false } }
        do {
            let response = try await fetchCandidates(requestedQuery, cursor)
            try Task.checkCancellation()
            guard generation == searchGeneration else { return }
            if cursor == nil { members = [] }
            let existingIds = Set(members.map(\.id))
            members.append(contentsOf: response.items.filter { !existingIds.contains($0.id) })
            nextCursor = response.pageInfo.nextCursor
        } catch where error.isUserCancellation {
        } catch {
            if generation == searchGeneration && !Task.isCancelled {
                errorMessage = error.actionMessage
                loadFailed = true
            }
        }
    }

    func add() async -> Bool {
        guard canAdd, let selectedId else { return false }
        errorMessage = nil
        loadFailed = false
        do {
            try await administration.addAdminProjectMember(projectId: projectId, userId: selectedId, role: role)
            return true
        } catch {
            errorMessage = error.actionMessage
            return false
        }
    }
}
