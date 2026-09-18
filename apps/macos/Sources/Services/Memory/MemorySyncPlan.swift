import CryptoKit
import Foundation

enum MemorySyncPlan {

    nonisolated static func reconciledOrgResources(
        existing: [MemoryResource],
        authoritative: [MemoryResource]
    ) -> [MemoryResource] {
        let existingById = Dictionary(
            existing.lazy.filter { $0.scope == .org }.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        return authoritative.filter { $0.scope == .org }.map { authority in
            guard let current = existingById[authority.id],
                  current.kind == authority.kind,
                  current.document.path == authority.document.path,
                  current.contentHash == authority.contentHash,
                  current.contentLoaded,
                  Self.contentHash(current.document.body) == current.contentHash else {
                return authority
            }
            var preserved = authority
            preserved.contentLoaded = true
            preserved.document.body = current.document.body
            return preserved
        }
    }

    nonisolated static func staleResourcePlan(
        displayedResources: [MemoryResource],
        projectName: String,
        observedProjectRefCommitId: String?,
        observedSelectedOrgResourceIds: Set<String> = [],
        observedOrgSelectionRevision: Int = 0,
        authoritativeCommitId: String?,
        serverCursor: String?,
        checkout: DaemonProjectCheckout,
        authoritativeRefEtag: String? = nil,
        authoritativeResponseIsStale: Bool = false,
        authoritativeOrgResources: [MemoryResource]? = nil,
        authoritativeOrgRefCommitId: String? = nil,
        authoritativeOrgResponseIsStale: Bool = false,
        provisionalResourceIds: Set<String> = [],
        generation: UUID = UUID()
    ) -> [String: StaleResourceSyncSnapshot]? {
        // A cursor mismatch has no direction information. Only a fresh Server
        // commit-state response can prove that the installed checkout is the
        // current shared version rather than an older checkout catching up.
        guard !authoritativeResponseIsStale,
              checkout.ready,
              let authoritativeCommitId,
              serverCursor == authoritativeCommitId,
              checkout.commitId == authoritativeCommitId else {
            return nil
        }
        guard observedProjectRefCommitId != authoritativeCommitId else { return [:] }

        let projectId = checkout.projectId
        let localProjectResources = displayedResources.filter {
            $0.scope == .project
                && $0.projectId == projectId
                && !provisionalResourceIds.contains($0.id)
        }
        let selectedOrgResourceIds = Set(checkout.selectedOrgResourceIds)
        let checkoutOrgResources = checkout.resources.filter { $0.scope == .org }
        let checkoutOrgResourceIds = Set(checkoutOrgResources.map(\.resourceId))
        guard checkoutOrgResourceIds == selectedOrgResourceIds else { return nil }

        // A Project checkout proves which Org blobs were materialized for that
        // Project, but absence from the checkout can mean either an Org delete
        // or a harmless Project deselection. Only a fresh, stable Org listing
        // can distinguish those cases and prove the checkout body still points
        // forward to the current Org authority.
        let orgIdsRequiringAuthority = observedSelectedOrgResourceIds
            .union(selectedOrgResourceIds)
        let authoritativeOrgById: [String: MemoryResource]
        if orgIdsRequiringAuthority.isEmpty {
            authoritativeOrgById = [:]
        } else {
            guard !authoritativeOrgResponseIsStale,
                  let authoritativeOrgResources,
                  let authoritativeOrgRefCommitId else {
                return nil
            }
            authoritativeOrgById = Dictionary(
                authoritativeOrgResources.compactMap { resource in
                    guard resource.scope == .org,
                          resource.projectId == nil,
                          resource.refCommitId == authoritativeOrgRefCommitId else {
                        return nil
                    }
                    return (resource.id, resource)
                },
                uniquingKeysWith: { _, latest in latest }
            )
        }

        let checkoutOrgById = Dictionary(
            checkoutOrgResources.map { ($0.resourceId, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        for resourceId in selectedOrgResourceIds {
            guard let checkoutResource = checkoutOrgById[resourceId],
                  let authority = authoritativeOrgById[resourceId],
                  authority.kind == .init(checkoutResource.resourceKind),
                  authority.document.path == checkoutResource.path,
                  authority.contentHash == checkoutResource.contentHash,
                  Self.contentHash(checkoutResource.content.content)
                    == checkoutResource.contentHash else {
                return nil
            }
        }

        // A resource removed from Project selection remains Org authority and
        // must stay in the global collection. It becomes a deletion candidate
        // only when the authoritative Org listing also says it is gone.
        let deletedSelectedOrgResourceIds = observedSelectedOrgResourceIds
            .subtracting(selectedOrgResourceIds)
            .filter { authoritativeOrgById[$0] == nil }
        let relevantOrgResourceIds = selectedOrgResourceIds
            .union(deletedSelectedOrgResourceIds)
        let localOrgResources = displayedResources.filter {
            $0.scope == .org
                && relevantOrgResourceIds.contains($0.id)
                && !provisionalResourceIds.contains($0.id)
        }
        let localResources = localProjectResources + localOrgResources
        let checkoutResources = checkout.resources.filter { $0.scope == .project }
        let localById = Dictionary(
            localResources.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        var remoteById = Dictionary(
            checkoutResources.map { resource -> (String, MemoryResource) in
                let local = localById[resource.resourceId]
                return (
                    resource.resourceId,
                    MemoryResource(
                        id: resource.resourceId,
                        scope: .project,
                        projectId: projectId,
                        projectName: projectName,
                        kind: .init(resource.resourceKind),
                        contentHash: resource.contentHash,
                        updatedAt: checkout.commitCreatedAt ?? local?.updatedAt ?? "",
                        refCommitId: authoritativeCommitId,
                        contentLoaded: true,
                        document: .init(
                            title: URL(fileURLWithPath: resource.path)
                                .deletingPathExtension().lastPathComponent,
                            path: resource.path,
                            body: resource.content.content
                        )
                    )
                )
            },
            uniquingKeysWith: { _, latest in latest }
        )
        for resourceId in selectedOrgResourceIds {
            guard let checkoutResource = checkoutOrgById[resourceId],
                  let authority = authoritativeOrgById[resourceId] else {
                return nil
            }
            remoteById[resourceId] = MemoryResource(
                id: resourceId,
                scope: .org,
                projectId: nil,
                projectName: nil,
                kind: authority.kind,
                contentHash: authority.contentHash,
                updatedAt: authority.updatedAt,
                refCommitId: authority.refCommitId,
                contentLoaded: true,
                document: .init(
                    title: authority.document.title,
                    path: authority.document.path,
                    body: checkoutResource.content.content
                )
            )
        }
        let projectRemoteIds = Set(checkoutResources.map(\.resourceId))
        let ids = Set(localResources.map(\.id))
            .union(projectRemoteIds)
            .union(selectedOrgResourceIds)
        var result: [String: StaleResourceSyncSnapshot] = [:]
        for id in ids {
            let local = localById[id]
            let remote = remoteById[id]
            if let local, let remote,
               local.scope == remote.scope,
               local.kind == remote.kind,
               local.document.path == remote.document.path,
               local.contentHash == remote.contentHash,
               (local.scope == .project
                    || !local.contentLoaded
                    || Self.contentHash(local.document.body) == local.contentHash) {
                continue
            }
            result[id] = .init(
                projectId: projectId,
                observedProjectRefCommitId: observedProjectRefCommitId,
                observedSelectedOrgResourceIds: observedSelectedOrgResourceIds,
                observedOrgSelectionRevision: observedOrgSelectionRevision,
                authoritativeCommitId: authoritativeCommitId,
                authoritativeRefEtag: authoritativeRefEtag ?? checkout.refEtag,
                selectedOrgResourceIds: Set(checkout.selectedOrgResourceIds),
                orgSelectionRevision: checkout.orgSelectionRevision,
                generation: generation,
                local: local,
                remote: remote
            )
        }
        return result
    }

    nonisolated static func staleResourcePlansMatch(
        _ lhs: [String: StaleResourceSyncSnapshot],
        _ rhs: [String: StaleResourceSyncSnapshot]
    ) -> Bool {
        guard Set(lhs.keys) == Set(rhs.keys) else { return false }
        return lhs.allSatisfy { resourceId, left in
            guard let right = rhs[resourceId],
                  left.local?.contentLoaded != false,
                  right.local?.contentLoaded != false else { return false }
            return left.projectId == right.projectId
                && left.observedProjectRefCommitId == right.observedProjectRefCommitId
                && left.observedSelectedOrgResourceIds == right.observedSelectedOrgResourceIds
                && left.observedOrgSelectionRevision == right.observedOrgSelectionRevision
                && left.authoritativeCommitId == right.authoritativeCommitId
                && left.authoritativeRefEtag == right.authoritativeRefEtag
                && left.selectedOrgResourceIds == right.selectedOrgResourceIds
                && left.orgSelectionRevision == right.orgSelectionRevision
                && left.local == right.local
                && left.remote == right.remote
        }
    }

    nonisolated static func contentHash(_ content: String) -> String {
        let digest = SHA256.hash(data: Data(content.utf8))
        return "sha256:" + digest.map { String(format: "%02x", $0) }.joined()
    }
}

struct StaleResourceSyncSnapshot: Equatable, Sendable {
    let projectId: String
    let observedProjectRefCommitId: String?
    let observedSelectedOrgResourceIds: Set<String>
    let observedOrgSelectionRevision: Int
    let authoritativeCommitId: String
    let authoritativeRefEtag: String?
    let selectedOrgResourceIds: Set<String>
    let orgSelectionRevision: Int
    let generation: UUID
    let local: MemoryResource?
    let remote: MemoryResource?
}
