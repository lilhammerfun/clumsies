import Combine
import Foundation

struct PendingBundleSave {
    let bundle: PersonalBundle
    let name: String
    let description: String
    let resourceIds: Set<String>
    let generation: UUID
}

@MainActor
final class BundleStore: ObservableObject {
    let didLoad = PassthroughSubject<Void, Never>()
    private let catalog: MemoryCatalog
    private let context: WorkspaceContext
    private let feedback: WorkspaceFeedback

    init(catalog: MemoryCatalog, context: WorkspaceContext, feedback: WorkspaceFeedback) {
        self.catalog = catalog
        self.context = context
        self.feedback = feedback
    }

    @Published var bundles: [PersonalBundle] = []
    @Published var bundleLoadState: WorkspaceCollectionLoadState = .loading

    private var bundleLoadTask: Task<Void, Never>?

    private let bundleMutationGate = AsyncMutex()

    var pendingBundleSaves: [String: PendingBundleSave] = [:]

    private var bundleSaveTasks: [String: Task<Void, Never>] = [:]

    nonisolated static func filterBundles(
        _ bundles: [PersonalBundle],
        query: String
    ) -> [PersonalBundle] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        guard !needle.isEmpty else { return bundles }
        return bundles.filter {
            "\($0.name) \($0.description)".localizedLowercase.contains(needle)
        }
    }

    func stageBundleSave(
        _ bundle: PersonalBundle,
        name: String,
        description: String,
        resourceIds: Set<String>
    ) {
        guard context.phase == .ready, !context.isSigningOut else { return }
        let generation = UUID()
        pendingBundleSaves[bundle.id] = .init(
            bundle: bundle,
            name: name,
            description: description,
            resourceIds: resourceIds,
            generation: generation
        )
        bundleSaveTasks[bundle.id]?.cancel()
        bundleSaveTasks[bundle.id] = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(600))
            guard !Task.isCancelled else { return }
            await self?.persistBundleSave(bundle.id, generation: generation)
        }
    }

    func flushBundleSave(_ bundleId: String) async throws {
        bundleSaveTasks[bundleId]?.cancel()
        bundleSaveTasks[bundleId] = nil
        guard let pending = pendingBundleSaves[bundleId] else { return }
        try await updateBundle(
            pending.bundle,
            name: pending.name,
            description: pending.description,
            resourceIds: pending.resourceIds
        )
        if pendingBundleSaves[bundleId]?.generation == pending.generation {
            pendingBundleSaves[bundleId] = nil
        }
    }

    func cancelBundleSave(_ bundleId: String) {
        bundleSaveTasks[bundleId]?.cancel()
        bundleSaveTasks[bundleId] = nil
        pendingBundleSaves[bundleId] = nil
    }

    func updateBundle(
        _ bundle: PersonalBundle,
        name: String,
        description: String,
        resourceIds: Set<String>
    ) async throws {
        let authority = context.authorityGeneration
        try await withBundleMutation {
            guard let current = bundles.first(where: { $0.id == bundle.id }) else { return }
            try self.catalog.validateOrgResourceIds(resourceIds)
            let selected = self.catalog.resources.filter {
                $0.scope == .org && resourceIds.contains($0.id)
            }
            let request = PersonalBundleRequest(
                name: name,
                description: description,
                resourceIds: selected.map(\.id)
            )
            let detail: PersonalBundleDetail = try await self.context.server.send(
                method: "PATCH",
                path: "/api/v1/me/bundles/\(bundle.id)",
                headers: ["If-Match": String(current.revision)],
                body: request
            )
            try self.context.ensureAuthority(authority)
            let updated = PersonalBundle(
                id: detail.bundle.bundleId,
                name: detail.bundle.name,
                description: detail.bundle.description,
                resourceIds: detail.memories.map(\.memoryId),
                revision: detail.bundle.revision,
                updatedAt: detail.bundle.updatedAt
            )
            if let index = bundles.firstIndex(where: { $0.id == updated.id }) {
                self.bundles[index] = updated
            }
        }
    }

    func withBundleMutation<T>(_ operation: () async throws -> T) async throws -> T {
        let authority = context.authorityGeneration
        await bundleMutationGate.lock()
        do {
            try context.ensureAuthority(authority)
            let result = try await operation()
            await bundleMutationGate.unlock()
            return result
        } catch {
            await bundleMutationGate.unlock()
            throw error
        }
    }

    private func persistBundleSave(_ bundleId: String, generation: UUID) async {
        guard let pending = pendingBundleSaves[bundleId], pending.generation == generation else { return }
        do {
            try await updateBundle(
                pending.bundle,
                name: pending.name,
                description: pending.description,
                resourceIds: pending.resourceIds
            )
            if pendingBundleSaves[bundleId]?.generation == generation {
                pendingBundleSaves[bundleId] = nil
                bundleSaveTasks[bundleId] = nil
            }
        } catch is CancellationError {
            return
        } catch {
            if pendingBundleSaves[bundleId]?.generation == generation {
                bundleSaveTasks[bundleId] = nil
                feedback.errorMessage = error.localizedDescription
            }
        }
    }

    func createBundleRecord() async -> String? {
        let authority = context.authorityGeneration
        do {
            return try await withBundleMutation {
                let detail: PersonalBundleDetail = try await self.context.server.send(
                    method: "POST",
                    path: "/api/v1/me/bundles",
                    body: PersonalBundleRequest(
                        name: "Untitled Bundle",
                        description: "",
                        resourceIds: []
                    )
                )
                try self.context.ensureAuthority(authority)
                let bundle = PersonalBundle(
                    id: detail.bundle.bundleId,
                    name: detail.bundle.name,
                    description: detail.bundle.description,
                    resourceIds: [],
                    revision: detail.bundle.revision,
                    updatedAt: detail.bundle.updatedAt
                )
                self.bundles.insert(bundle, at: 0)
                return bundle.id
            }
        } catch {
            guard context.authorityGeneration == authority, !(error is CancellationError) else { return nil }
            feedback.errorMessage = error.localizedDescription
            return nil
        }
    }

    func deleteBundleRecord(_ bundle: PersonalBundle) async -> Bool {
        let authority = context.authorityGeneration
        do {
            return try await withBundleMutation {
                guard let current = bundles.first(where: { $0.id == bundle.id }) else { return false }
                let response = try await self.context.server.raw(
                    method: "DELETE",
                    path: "/api/v1/me/bundles/\(bundle.id)",
                    headers: ["If-Match": String(current.revision)]
                )
                try self.context.ensureAuthority(authority)
                guard (200..<300).contains(response.status) else {
                    throw ServerClientError.response(status: response.status, message: response.body)
                }
                self.bundles.removeAll { $0.id == bundle.id }
                return true
            }
        } catch {
            guard context.authorityGeneration == authority, !(error is CancellationError) else { return false }
            feedback.errorMessage = error.localizedDescription
            return false
        }
    }

    func startLoading(
        generation: UUID,
        requiresFreshData: Bool,
        baseSnapshotWasStale: Bool
    ) {
        cancelLoading()
        let loader = context.loader
        let baselineBundles = bundles
        bundleLoadState = .loading
        bundleLoadTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.context.workspaceReloadGeneration == generation {
                    self.bundleLoadTask = nil
                }
            }
            do {
                let loaded = try await loader.loadBundles()
                try Task.checkCancellation()
                guard let self,
                      context.workspaceReloadGeneration == generation,
                      context.phase == .ready else {
                    return
                }
                guard WorkspaceLoadPolicy.canPublishDeferredLoad(
                    requiresFreshData: requiresFreshData,
                    baseSnapshotWasStale: baseSnapshotWasStale,
                    responseWasStale: loaded.hasStaleServerResponse
                ) else {
                    bundleLoadState = .failed(
                        "Fresh Bundle data was unavailable. Existing Bundles were kept."
                    )
                    return
                }
                bundles = WorkspaceLoadPolicy.mergeDeferredRecords(
                    baseline: baselineBundles,
                    current: bundles,
                    loaded: loaded.records
                )
                bundleLoadState = .loaded
                didLoad.send()
            } catch is CancellationError {
                return
            } catch {
                guard let self, context.workspaceReloadGeneration == generation else { return }
                bundleLoadState = .failed(error.localizedDescription)
            }
        }
    }

    func cancelLoading() {
        bundleLoadTask?.cancel()
        bundleLoadTask = nil
    }

    var hasPendingChanges: Bool { !pendingBundleSaves.isEmpty }

    func flushPendingChanges() async throws {
        for bundleId in Array(pendingBundleSaves.keys) {
            try await flushBundleSave(bundleId)
        }
    }

    func resetAuthority() {
        cancelLoading()
        bundleSaveTasks.values.forEach { $0.cancel() }
        bundleSaveTasks.removeAll()
        pendingBundleSaves.removeAll()
        bundles.removeAll()
        bundleLoadState = .loading
    }
}
