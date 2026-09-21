import Combine
import Foundation

@MainActor
final class MemorySyncService: ObservableObject {
    private let catalog: MemoryCatalog
    private let context: WorkspaceContext
    private let feedback: WorkspaceFeedback

    init(catalog: MemoryCatalog, context: WorkspaceContext, feedback: WorkspaceFeedback) {
        self.catalog = catalog
        self.context = context
        self.feedback = feedback
    }

    func refreshOrgResourcesIfNeeded(isActive: () -> Bool) async {
        let workspaceGeneration = context.workspaceReloadGeneration
        guard isActive(),
              context.activeProjectId == nil,
              !context.isSwitchingMemoryContext,
              catalog.orgResourceRefreshGeneration == nil else {
            return
        }
        let observedOrgRefCommitId = catalog.orgRefCommitId
        let generation = UUID()
        catalog.orgResourceRefreshGeneration = generation
        defer {
            if self.catalog.orgResourceRefreshGeneration == generation {
                self.catalog.orgResourceRefreshGeneration = nil
            }
        }
        do {
            let head: (value: CommitStateResponse, response: DaemonServerResponse) =
                try await context.server.getWithMetadata("/api/v1/org/commit-state")
            guard context.workspaceReloadGeneration == workspaceGeneration,
                  context.phase == .ready,
                  isActive(),
                  context.activeProjectId == nil,
                  !context.isSwitchingMemoryContext,
                  catalog.orgResourceRefreshGeneration == generation,
                  !head.response.isStaleCache else {
                return
            }
            guard head.value.ref.commitId != observedOrgRefCommitId else {
                feedback.resolveBackgroundError(.organizationResources)
                return
            }
            guard let snapshot = try await catalog.loadStableOrgAuthoritySnapshot(),
                  let snapshotCommitId = snapshot.commitId,
                  snapshotCommitId == head.value.ref.commitId,
                  context.workspaceReloadGeneration == workspaceGeneration,
                  context.phase == .ready,
                  isActive(),
                  context.activeProjectId == nil,
                  !self.context.isSwitchingMemoryContext,
                  catalog.orgResourceRefreshGeneration == generation,
                  catalog.orgRefCommitId == observedOrgRefCommitId else {
                return
            }

            // Any inactive Project plan containing an Org row was derived
            // from an older global authority generation. Drop the whole plan
            // before installing the new Org snapshot so returning to that
            // Project can never apply an old checkout and advance its ref.
            let projectsWithOrgStalePlans = Set(catalog.staleResourceSnapshots.values.compactMap {
                snapshot in
                snapshot.local?.scope == .org || snapshot.remote?.scope == .org
                    ? snapshot.projectId
                    : nil
            })
            for projectId in projectsWithOrgStalePlans {
                catalog.clearStaleResourceState(for: projectId)
            }
            let previousOrgResources = catalog.resources.filter { $0.scope == .org }
            let nextOrgResources = MemorySyncPlan.reconciledOrgResources(
                existing: previousOrgResources,
                authoritative: snapshot.resources
            )
            let previousById = Dictionary(
                previousOrgResources.map { ($0.id, $0) },
                uniquingKeysWith: { _, latest in latest }
            )
            let nextById = Dictionary(
                nextOrgResources.map { ($0.id, $0) },
                uniquingKeysWith: { _, latest in latest }
            )

            // Publish the complete authority generation in one assignment so
            // the tree cannot observe a mixed old/new Org catalog.
            catalog.resources = catalog.resources.filter { $0.scope != .org } + nextOrgResources
            catalog.orgRefCommitId = snapshotCommitId
            catalog.orgRefEtag = snapshot.refEtag ?? ""
            for resourceId in Set(previousById.keys).union(nextById.keys) {
                let previous = previousById[resourceId]
                let next = nextById[resourceId]
                if previous?.kind != next?.kind
                    || previous?.contentHash != next?.contentHash
                    || previous?.contentLoaded != next?.contentLoaded
                    || previous?.document != next?.document {
                    catalog.bumpDocumentContentGeneration(for: resourceId)
                }
            }
            catalog.documentsChanged.send()
            feedback.resolveBackgroundError(.organizationResources)
        } catch where error.isUserCancellation {
            return
        } catch {
            guard context.workspaceReloadGeneration == workspaceGeneration,
                  context.phase == .ready,
                  isActive(),
                  context.activeProjectId == nil,
                  !context.isSwitchingMemoryContext,
                  catalog.orgResourceRefreshGeneration == generation else {
                return
            }
            feedback.presentBackgroundError(
                error,
                source: .organizationResources
            )
        }
    }

    func refreshStaleResourcesIfNeeded(sync: DaemonSyncStatus) async {
        let workspaceGeneration = context.workspaceReloadGeneration
        guard let projectId = context.activeProjectId,
              let project = context.projects.first(where: { $0.id == projectId }),
              let serverCursor = sync.commitSync.serverCursor else {
            return
        }
        let errorSource = WorkspaceBackgroundErrorSource.staleResources(projectId: projectId)
        guard serverCursor != project.refCommitId else {
            feedback.resolveBackgroundError(errorSource)
            return
        }
        let observedRef = project.refCommitId
        let refreshGeneration = UUID()
        catalog.staleResourceRefreshGenerations[projectId] = refreshGeneration
        do {
            let commit: (value: CommitStateResponse, response: DaemonServerResponse) =
                try await context.server.getWithMetadata("/api/v1/projects/\(projectId)/commit-state")
            guard context.activeProjectId == projectId,
                  context.projects.first(where: { $0.id == projectId }) == project,
                  catalog.staleResourceRefreshGenerations[projectId] == refreshGeneration,
                  !commit.response.isStaleCache else {
                return
            }
            let authoritativeCommitId = commit.value.ref.commitId
            let authoritativeRefEtag = commit.response.headers.first {
                $0.key.caseInsensitiveCompare("etag") == .orderedSame
            }?.value
            if authoritativeCommitId == observedRef {
                catalog.clearStaleResourceState(for: projectId)
                if let authoritativeCommitId {
                    catalog.advanceProjectRefIfPlanCompleted(
                        projectId: projectId,
                        authoritativeCommitId: authoritativeCommitId,
                        authoritativeRefEtag: authoritativeRefEtag,
                        selectedOrgResourceIds: project.selectedOrgResourceIds,
                        orgSelectionRevision: project.orgSelectionRevision
                    )
                }
                feedback.resolveBackgroundError(errorSource)
                return
            }
            let checkout = try await context.daemon.projectCheckout(projectId)
            guard context.activeProjectId == projectId,
                  context.projects.first(where: { $0.id == projectId }) == project,
                  catalog.staleResourceRefreshGenerations[projectId] == refreshGeneration else {
                return
            }
            let needsOrgAuthority = !project.selectedOrgResourceIds.isEmpty
                || !checkout.selectedOrgResourceIds.isEmpty
            let orgAuthority: (
                commitId: String,
                refEtag: String?,
                resources: [MemoryResource]
            )?
            if needsOrgAuthority {
                guard let snapshot = try await catalog.loadStableOrgAuthoritySnapshot(),
                      let commitId = snapshot.commitId else { return }
                orgAuthority = (commitId, snapshot.refEtag, snapshot.resources)
            } else {
                orgAuthority = nil
            }
            let verifiedCommit: (value: CommitStateResponse, response: DaemonServerResponse) =
                try await context.server.getWithMetadata(
                    "/api/v1/projects/\(projectId)/commit-state"
                )
            guard context.activeProjectId == projectId,
                  context.projects.first(where: { $0.id == projectId }) == project,
                  catalog.staleResourceRefreshGenerations[projectId] == refreshGeneration,
                  !verifiedCommit.response.isStaleCache,
                  verifiedCommit.value.ref.commitId == authoritativeCommitId,
                  let plan = MemorySyncPlan.staleResourcePlan(
                    displayedResources: catalog.resources,
                    projectName: project.name,
                    observedProjectRefCommitId: observedRef,
                    observedSelectedOrgResourceIds: project.selectedOrgResourceIds,
                    observedOrgSelectionRevision: project.orgSelectionRevision,
                    authoritativeCommitId: authoritativeCommitId,
                    serverCursor: serverCursor,
                    checkout: checkout,
                    authoritativeRefEtag: authoritativeRefEtag,
                    authoritativeResponseIsStale: commit.response.isStaleCache,
                    authoritativeOrgResources: orgAuthority?.resources,
                    authoritativeOrgRefCommitId: orgAuthority?.commitId,
                    // Remote-only additions are inserted provisionally so the
                    // file tree can expose their Sync action. They are not
                    // part of the observed local generation and must not make
                    // the next poll conclude that the plan is already applied.
                    provisionalResourceIds: catalog.provisionalStaleAdditionIds
                  ) else {
                return
            }
            let installedPlan = catalog.staleResourceSnapshots.filter { _, snapshot in
                snapshot.projectId == projectId
            }
            if !plan.isEmpty, MemorySyncPlan.staleResourcePlansMatch(plan, installedPlan) {
                feedback.resolveBackgroundError(errorSource)
                return
            }
            let hydratedPlan = await hydrateStaleResourcePlan(plan)
            guard context.activeProjectId == projectId,
                  context.projects.first(where: { $0.id == projectId }) == project,
                  catalog.staleResourceRefreshGenerations[projectId] == refreshGeneration else {
                return
            }
            catalog.installStaleResourcePlan(hydratedPlan, for: projectId)
            if hydratedPlan.isEmpty {
                catalog.advanceProjectRefIfPlanCompleted(
                    projectId: projectId,
                    authoritativeCommitId: authoritativeCommitId ?? serverCursor,
                    authoritativeRefEtag: authoritativeRefEtag ?? checkout.refEtag,
                    selectedOrgResourceIds: Set(checkout.selectedOrgResourceIds),
                    orgSelectionRevision: checkout.orgSelectionRevision
                )
            }
            feedback.resolveBackgroundError(errorSource)
        } catch where error.isUserCancellation {
            return
        } catch {
            guard context.workspaceReloadGeneration == workspaceGeneration,
                  context.phase == .ready,
                  context.activeProjectId == projectId,
                  context.projects.first(where: { $0.id == projectId }) == project,
                  catalog.staleResourceRefreshGenerations[projectId] == refreshGeneration else {
                return
            }
            feedback.presentBackgroundError(
                error,
                source: errorSource
            )
        }
    }

    private func hydrateStaleResourcePlan(
        _ plan: [String: StaleResourceSyncSnapshot]
    ) async -> [String: StaleResourceSyncSnapshot] {
        var hydrated = plan
        var payloads: [String: CommitPayload] = [:]
        var unavailableCommitIds = Set<String>()

        for (resourceId, snapshot) in plan {
            guard var local = snapshot.local else { continue }
            // Some files may already have applied an earlier generation while
            // the Project ref intentionally remains at the all-files barrier.
            // Hydrate each file from its own generation first.
            let commitId = local.refCommitId ?? snapshot.observedProjectRefCommitId
            var historicalBody: String?
            if let commitId, !unavailableCommitIds.contains(commitId) {
                let payload: CommitPayload?
                if let cached = payloads[commitId] {
                    payload = cached
                } else {
                    do {
                        let loaded = try await catalog.loadCommit(commitId)
                        if let loaded {
                            payloads[commitId] = loaded
                        } else {
                            unavailableCommitIds.insert(commitId)
                        }
                        payload = loaded
                    } catch {
                        unavailableCommitIds.insert(commitId)
                        payload = nil
                    }
                }
                if let payload,
                   let entry = payload.tree.entries.first(where: { entry in
                    entry.type == .memory && entry.id == local.id
                   }),
                   let blob = payload.blobs.first(where: { $0.blobId == entry.blobId }),
                   MemorySyncPlan.contentHash(blob.content) == local.contentHash {
                    historicalBody = blob.content
                }
            }

            if let historicalBody {
                local.document.body = historicalBody
                local.contentLoaded = true
            } else if MemorySyncPlan.contentHash(local.document.body) == local.contentHash {
                // A loaded body is only an acceptable fallback when its hash
                // proves it belongs to the observed generation.
                local.contentLoaded = true
            } else {
                local.document.body = ""
                local.contentLoaded = false
            }
            hydrated[resourceId] = .init(
                projectId: snapshot.projectId,
                observedProjectRefCommitId: snapshot.observedProjectRefCommitId,
                observedSelectedOrgResourceIds: snapshot.observedSelectedOrgResourceIds,
                observedOrgSelectionRevision: snapshot.observedOrgSelectionRevision,
                authoritativeCommitId: snapshot.authoritativeCommitId,
                authoritativeRefEtag: snapshot.authoritativeRefEtag,
                selectedOrgResourceIds: snapshot.selectedOrgResourceIds,
                orgSelectionRevision: snapshot.orgSelectionRevision,
                generation: snapshot.generation,
                local: local,
                remote: snapshot.remote
            )
        }
        return hydrated
    }
}
