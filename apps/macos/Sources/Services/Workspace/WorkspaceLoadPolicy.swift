import Combine
import Foundation

enum WorkspaceLoadPolicy {

    nonisolated static func preservesDeferredAuthority(
        currentAccount: UserReference?,
        currentOrganization: OrganizationReference?,
        nextAccount: UserReference,
        nextOrganization: OrganizationReference
    ) -> Bool {
        currentAccount?.userId == nextAccount.userId
            && currentOrganization?.orgId == nextOrganization.orgId
    }

    nonisolated static func invalidateWorkspaceTransitionState(
        generation: inout UUID,
        loadingProjectId: inout String?,
        isSwitchingMemoryContext: inout Bool,
        isPreparingWorkspaceIndex: inout Bool,
        orgResourceRefreshGeneration: inout UUID?
    ) {
        generation = UUID()
        loadingProjectId = nil
        isSwitchingMemoryContext = false
        isPreparingWorkspaceIndex = false
        orgResourceRefreshGeneration = nil
    }

    nonisolated static func rejectsStaleAuthorityChange(
        hadLoadedWorkspace: Bool,
        sameAuthority: Bool,
        snapshotWasStale: Bool
    ) -> Bool {
        hadLoadedWorkspace && !sameAuthority && snapshotWasStale
    }

    nonisolated static func deferredLoadRequiresFreshData(
        hadLoadedWorkspace: Bool
    ) -> Bool {
        hadLoadedWorkspace
    }

    nonisolated static func retainingAccessibleProjectRecords<Record>(
        _ records: [Record],
        accessibleProjectIds: Set<String>,
        projectId: KeyPath<Record, String>
    ) -> [Record] {
        records.filter { accessibleProjectIds.contains($0[keyPath: projectId]) }
    }

    nonisolated static func canPublishDeferredLoad(
        requiresFreshData: Bool,
        baseSnapshotWasStale: Bool,
        responseWasStale: Bool
    ) -> Bool {
        !requiresFreshData || (!baseSnapshotWasStale && !responseWasStale)
    }

    nonisolated static func mergeDeferredRecords<Record>(
        baseline: [Record],
        current: [Record],
        loaded: [Record]
    ) -> [Record] where Record: Identifiable & Equatable, Record.ID: Hashable {
        let baselineById = Dictionary(
            baseline.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        let currentById = Dictionary(
            current.map { ($0.id, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        let loadedIds = Set(loaded.map(\.id))
        var merged = current.filter {
            !loadedIds.contains($0.id) && currentById[$0.id] != baselineById[$0.id]
        }
        merged.append(contentsOf: loaded.compactMap { loadedRecord -> Record? in
            let id = loadedRecord.id
            guard currentById[id] != baselineById[id] else { return loadedRecord }
            return currentById[id]
        })
        return merged
    }
}
