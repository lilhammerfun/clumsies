import Combine
import Foundation

struct ResourceLoadRequest: Sendable {
    let resource: MemoryResource
    let generation: UUID
    let task: Task<String?, Never>
}

@MainActor
final class MemoryCatalog: ObservableObject {
    private let context: WorkspaceContext
    let documentsChanged = PassthroughSubject<Void, Never>()

    init(context: WorkspaceContext) {
        self.context = context
    }

    @Published var orgRefCommitId: String?
    @Published var orgRefEtag = ""
    @Published var resources: [MemoryResource] = []
    @Published var loadingResourceIds: Set<String> = []
    @Published var isPreparingWorkspaceIndex = false

    /// Project resources whose shared version moved forward after the app
    /// loaded its snapshot; they show a sync icon and can be refreshed.
    @Published var staleResourceIds: Set<String> = []

    /// Incremented when authoritative content replaces the document currently
    /// held by a long-lived editor session.
    @Published var documentContentGenerations: [String: UInt64] = [:]

    var staleResourceSnapshots: [String: StaleResourceSyncSnapshot] = [:]

    var provisionalStaleAdditionIds: Set<String> = []

    var resourceLoadRequests: [String: ResourceLoadRequest] = [:]

    var staleResourceRefreshGenerations: [String: UUID] = [:]

    var orgResourceRefreshGeneration: UUID?

    func documentContentGeneration(for itemId: String) -> UInt64 {
        documentContentGenerations[itemId, default: 0]
    }

    func staleResourceGeneration(for resourceId: String) -> UUID? {
        guard let activeProjectId = context.activeProjectId,
              let snapshot = staleResourceSnapshots[resourceId],
              snapshot.projectId == activeProjectId else { return nil }
        return snapshot.generation
    }

    func staleResourceSnapshot(
        for item: MemoryListItem
    ) -> StaleResourceSyncSnapshot? {
        guard let resourceId = item.resource?.id,
              let projectId = item.projectContextId,
              projectId == context.activeProjectId,
              let snapshot = staleResourceSnapshots[resourceId],
              snapshot.projectId == projectId else { return nil }
        return snapshot
    }

    nonisolated static func resourceGenerationMatches(
        _ lhs: MemoryResource,
        _ rhs: MemoryResource
    ) -> Bool {
        lhs.id == rhs.id
            && lhs.scope == rhs.scope
            && lhs.projectId == rhs.projectId
            && lhs.kind == rhs.kind
            && lhs.contentHash == rhs.contentHash
            && lhs.refCommitId == rhs.refCommitId
            && lhs.document.path == rhs.document.path
    }

    @discardableResult
    func installLoadedResourceIfCurrent(_ loaded: MemoryResource) -> Bool {
        guard let index = resources.firstIndex(where: { $0.id == loaded.id }),
              !resources[index].contentLoaded,
              Self.resourceGenerationMatches(resources[index], loaded) else {
            return false
        }
        resources[index] = loaded
        bumpDocumentContentGeneration(for: loaded.id)
        return true
    }

    @discardableResult
    func loadContentIfNeeded(
        _ item: MemoryListItem,
        loadContent: (@Sendable (MemoryResource) async throws -> MemoryResource)? = nil
    ) async -> String? {
        guard !Task.isCancelled else { return nil }
        guard item.draft == nil,
              let resource = item.resource,
              !resource.contentLoaded else { return nil }
        if let inFlight = resourceLoadRequests[resource.id],
           Self.resourceGenerationMatches(inFlight.resource, resource) {
            return await inFlight.task.value
        }
        resourceLoadRequests[resource.id]?.task.cancel()
        if let snapshot = staleResourceSnapshot(for: item) {
            guard let local = snapshot.local, local.contentLoaded else {
                return DocumentDiffError.baselineUnavailable.localizedDescription
            }
            if let index = resources.firstIndex(where: { $0.id == resource.id }) {
                resources[index] = local
                bumpDocumentContentGeneration(for: resource.id)
            }
            return nil
        }
        let generation = UUID()
        let task = Task { @MainActor [weak self] in
            await self?.loadResourceContent(resource, generation: generation, loadContent: loadContent)
        }
        resourceLoadRequests[resource.id] = .init(resource: resource, generation: generation, task: task)
        loadingResourceIds.insert(resource.id)
        return await task.value
    }

    private func loadResourceContent(
        _ resource: MemoryResource,
        generation: UUID,
        loadContent: (@Sendable (MemoryResource) async throws -> MemoryResource)?
    ) async -> String? {
        defer {
            if resourceLoadRequests[resource.id]?.generation == generation {
                resourceLoadRequests.removeValue(forKey: resource.id)
                loadingResourceIds.remove(resource.id)
            }
        }
        do {
            let loaded: MemoryResource
            if let loadContent {
                loaded = try await loadContent(resource)
            } else {
                loaded = try await WorkspaceLoader(
                    daemon: context.daemon, bootstrap: context.bootstrap, server: context.server
                ).loadContent(for: resource)
            }
            try Task.checkCancellation()
            guard resourceLoadRequests[resource.id]?.generation == generation else { return nil }
            installLoadedResourceIfCurrent(loaded)
        } catch is CancellationError {
            return nil
        } catch {
            guard !Task.isCancelled,
                  resourceLoadRequests[resource.id]?.generation == generation else { return nil }
            return error.localizedDescription
        }
        return nil
    }

    nonisolated static func stableOrgAuthorityCommitId(
        beforeCommitId: String?,
        afterCommitId: String?,
        responseIsStale: Bool
    ) -> String? {
        guard !responseIsStale,
              let beforeCommitId,
              afterCommitId == beforeCommitId else {
            return nil
        }
        return beforeCommitId
    }

    func loadStableOrgAuthoritySnapshot(
        allowingEmptyHead: Bool = false
    ) async throws
        -> (commitId: String?, refEtag: String?, resources: [MemoryResource])? {
        let before: (value: CommitStateResponse, response: DaemonServerResponse) =
            try await context.server.getWithMetadata("/api/v1/org/commit-state")

        var metadata: [MemoryMetadata] = []
        var cursor: String?
        var listingIsStale = false
        repeat {
            var query = [URLQueryItem(name: "limit", value: "200")]
            if let cursor { query.append(.init(name: "cursor", value: cursor)) }
            let page: (value: ListResponse<MemoryMetadata>, response: DaemonServerResponse) =
                try await context.server.getWithMetadata("/api/v1/org/memories", query: query)
            listingIsStale = listingIsStale || page.response.isStaleCache
            metadata += page.value.items
            cursor = page.value.pageInfo.hasMore ? page.value.pageInfo.nextCursor : nil
        } while cursor != nil

        let after: (value: CommitStateResponse, response: DaemonServerResponse) =
            try await context.server.getWithMetadata("/api/v1/org/commit-state")
        let responseIsStale = before.response.isStaleCache
            || listingIsStale
            || after.response.isStaleCache
        let commitId = before.value.ref.commitId
        guard !responseIsStale,
              after.value.ref.commitId == commitId,
              allowingEmptyHead || commitId != nil else {
            return nil
        }
        let refEtag = after.response.headers.first {
            $0.key.caseInsensitiveCompare("etag") == .orderedSame
        }?.value ?? before.response.headers.first {
            $0.key.caseInsensitiveCompare("etag") == .orderedSame
        }?.value
        return (
            commitId,
            refEtag,
            metadata.map { item in
                MemoryResource(
                    id: item.memoryId,
                    scope: .org,
                    projectId: nil,
                    projectName: nil,
                    kind: .init(.memory),
                    contentHash: item.contentHash,
                    updatedAt: item.updatedAt,
                    refCommitId: commitId,
                    contentLoaded: false,
                    document: .init(title: item.name, path: item.path, body: "")
                )
            }
        )
    }

    func installStaleResourcePlan(
        _ plan: [String: StaleResourceSyncSnapshot],
        for projectId: String
    ) {
        let applicablePlan = plan.filter { resourceId, snapshot in
            if let local = snapshot.local {
                guard let current = resources.first(where: { $0.id == resourceId }) else {
                    return false
                }
                return Self.resourceGenerationMatches(current, local)
            }
            return !self.resources.contains(where: { $0.id == resourceId })
                || self.provisionalStaleAdditionIds.contains(resourceId)
        }
        clearStaleResourceState(for: projectId)
        for (resourceId, snapshot) in applicablePlan {
            staleResourceSnapshots[resourceId] = snapshot
            if let local = snapshot.local,
               local.contentLoaded,
               let index = resources.firstIndex(where: { $0.id == resourceId }),
               resources[index] != local {
                resources[index] = local
                bumpDocumentContentGeneration(for: resourceId)
            }
            if snapshot.local == nil,
               let remote = snapshot.remote,
               !resources.contains(where: { $0.id == resourceId }) {
                resources.append(remote)
                provisionalStaleAdditionIds.insert(resourceId)
            }
        }
        refreshVisibleStaleResourceIds()
    }

    func clearStaleResourceState(for projectId: String) {
        staleResourceRefreshGenerations[projectId] = UUID()
        let ids = Set(staleResourceSnapshots.compactMap { resourceId, snapshot in
            snapshot.projectId == projectId ? resourceId : nil
        })
        let provisionalIds = ids.intersection(provisionalStaleAdditionIds)
        if !provisionalIds.isEmpty {
            resources.removeAll { provisionalIds.contains($0.id) }
            provisionalStaleAdditionIds.subtract(provisionalIds)
            for resourceId in provisionalIds {
                bumpDocumentContentGeneration(for: resourceId)
            }
        }
        staleResourceSnapshots = staleResourceSnapshots.filter { _, snapshot in
            snapshot.projectId != projectId
        }
        refreshVisibleStaleResourceIds()
    }

    private func clearAllStaleResourceState() {
        if !provisionalStaleAdditionIds.isEmpty {
            resources.removeAll { self.provisionalStaleAdditionIds.contains($0.id) }
        }
        provisionalStaleAdditionIds.removeAll()
        staleResourceSnapshots.removeAll()
        staleResourceIds.removeAll()
        staleResourceRefreshGenerations.removeAll()
    }

    func refreshVisibleStaleResourceIds() {
        guard let activeProjectId = context.activeProjectId else {
            staleResourceIds.removeAll()
            return
        }
        staleResourceIds = Set(staleResourceSnapshots.compactMap { resourceId, snapshot in
            snapshot.projectId == activeProjectId ? resourceId : nil
        })
    }

    func advanceProjectRefIfPlanCompleted(
        projectId: String,
        authoritativeCommitId: String,
        authoritativeRefEtag: String?,
        selectedOrgResourceIds: Set<String>,
        orgSelectionRevision: Int
    ) {
        guard !staleResourceSnapshots.values.contains(where: { $0.projectId == projectId }),
              let index = context.projects.firstIndex(where: { $0.id == projectId }) else {
            return
        }
        let current = context.projects[index]
        context.projects[index] = .init(
            id: current.id,
            name: current.name,
            refCommitId: authoritativeCommitId,
            refEtag: authoritativeRefEtag ?? current.refEtag,
            selectedOrgResourceIds: selectedOrgResourceIds,
            orgSelectionRevision: orgSelectionRevision,
            isLoaded: current.isLoaded
        )
        for resourceIndex in resources.indices
        where resources[resourceIndex].scope == .project
            && resources[resourceIndex].projectId == projectId
            && resources[resourceIndex].refCommitId != authoritativeCommitId {
            let resource = resources[resourceIndex]
            resources[resourceIndex] = .init(
                id: resource.id,
                scope: resource.scope,
                projectId: resource.projectId,
                projectName: resource.projectName,
                kind: resource.kind,
                contentHash: resource.contentHash,
                updatedAt: resource.updatedAt,
                refCommitId: authoritativeCommitId,
                contentLoaded: resource.contentLoaded,
                document: resource.document
            )
        }
    }

    func bumpDocumentContentGeneration(for resourceId: String) {
        documentContentGenerations[resourceId, default: 0] &+= 1
    }

    func replaceProjectResources(
        projectId: String,
        with replacements: [MemoryResource]
    ) {
        let previous = Dictionary(
            resources.lazy.filter { $0.projectId == projectId }.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        resources.removeAll { $0.projectId == projectId }
        resources += replacements
        let next = Dictionary(
            replacements.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        for resourceId in Set(previous.keys).union(next.keys)
        where previous[resourceId] != next[resourceId] {
            bumpDocumentContentGeneration(for: resourceId)
        }
        documentsChanged.send()
    }

    func adoptCurrentStaleResource(_ resourceId: String) {
        guard let snapshot = staleResourceSnapshots.removeValue(forKey: resourceId) else { return }
        staleResourceRefreshGenerations[snapshot.projectId] = UUID()
        provisionalStaleAdditionIds.remove(resourceId)
        refreshVisibleStaleResourceIds()
        advanceProjectRefIfPlanCompleted(
            projectId: snapshot.projectId,
            authoritativeCommitId: snapshot.authoritativeCommitId,
            authoritativeRefEtag: snapshot.authoritativeRefEtag,
            selectedOrgResourceIds: snapshot.selectedOrgResourceIds,
            orgSelectionRevision: snapshot.orgSelectionRevision
        )
    }

    func validateOrgResourceIds(_ resourceIds: Set<String>) throws {
        let available = Set(resources.lazy.filter { $0.scope == .org }.map(\.id))
        guard resourceIds.isSubset(of: available) else {
            throw ProjectMemorySelectionError.invalidOrgResources
        }
    }

    func loadCommit(_ commitId: String?) async throws -> CommitPayload? {
        guard let commitId else { return nil }
        return try await context.server.get("/api/v1/commits/\(commitId)")
    }

    private func cancelContentLoads() {
        resourceLoadRequests.values.forEach { $0.task.cancel() }
        resourceLoadRequests.removeAll()
        loadingResourceIds.removeAll()
    }

    func resetAuthority() {
        cancelContentLoads()
        clearAllStaleResourceState()
        orgResourceRefreshGeneration = nil
        isPreparingWorkspaceIndex = false
        orgRefCommitId = nil
        orgRefEtag = ""
        resources.removeAll()
    }

    func apply(_ snapshot: WorkspaceSnapshot) {
        let previous = Dictionary(resources.map { ($0.id, $0) }, uniquingKeysWith: { _, latest in latest })
        cancelContentLoads()
        clearAllStaleResourceState()
        orgResourceRefreshGeneration = nil
        isPreparingWorkspaceIndex = false
        orgRefCommitId = snapshot.orgRefCommitId
        orgRefEtag = snapshot.orgRefEtag
        resources = snapshot.resources
        let next = Dictionary(resources.map { ($0.id, $0) }, uniquingKeysWith: { _, latest in latest })
        for id in Set(previous.keys).union(next.keys) where previous[id] != next[id] {
            bumpDocumentContentGeneration(for: id)
        }
    }
}

enum DocumentDiffError: LocalizedError, Equatable, Sendable {
    case baselineUnavailable

    var errorDescription: String? {
        switch self {
        case .baselineUnavailable:
            "The previous shared content is unavailable, so an accurate Diff cannot be shown."
        }
    }
}
