import Combine
import Foundation

enum MemoryTreeProjection {
    static func items(
        resources: [MemoryResource], drafts: [LocalDraft], activeProjectId: String?,
        selectedOrgResourceIds: Set<String>
    ) -> [MemoryListItem] {
        let authoritative = MemoryTreeProjection.memoryTreeResources(
            resources,
            activeProjectId: activeProjectId,
            selectedOrgResourceIds: selectedOrgResourceIds
        )
        let activeDrafts = MemoryTreeProjection.preferredMemoryTreeDrafts(
            MemoryTreeProjection.memoryTreeDrafts(drafts, activeProjectId: activeProjectId)
        )
        var draftByTarget: [String: LocalDraft] = [:]
        for draft in activeDrafts {
            guard let target = draft.targetId ?? draft.orgSource?.resourceId else { continue }
            if let current = draftByTarget[target] {
                if current.scope == .project && draft.scope == .org { continue }
                if current.scope == draft.scope && current.updatedAt >= draft.updatedAt { continue }
            }
            draftByTarget[target] = draft
        }
        var items = authoritative.map { resource in
            MemoryListItem(
                id: resource.id,
                resource: resource,
                draft: draftByTarget[resource.id],
                inherited: resource.scope == .org
                    && activeProjectId != nil
                    && selectedOrgResourceIds.contains(resource.id),
                projectContextId: activeProjectId
            )
        }
        let authoritativeIds = Set(authoritative.map(\.id))
        let representedDraftIds = Set(items.compactMap { $0.draft?.id })
        items.append(contentsOf: activeDrafts.filter { !representedDraftIds.contains($0.id) }.map {
            MemoryListItem(
                id: $0.targetId.flatMap { authoritativeIds.contains($0) ? nil : $0 } ?? $0.id,
                resource: nil,
                draft: $0,
                inherited: false,
                projectContextId: activeProjectId
            )
        })
        return items.sorted { $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending }
    }

    nonisolated static func filterMemoryItems(
        _ items: [MemoryListItem],
        query: String
    ) -> [MemoryListItem] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        guard !needle.isEmpty else { return items }
        return items.filter { item in
            let document = item.document
            return "\(document.title) \(document.path) \(document.body) \(item.kind.title)"
                .localizedLowercase.contains(needle)
        }
    }

    /// The Project tree is the Project's effective Memory surface: selected
    /// Org references, published Project Memory, and local Drafts.
    /// Explicit adaptations take precedence over their Org source.
    nonisolated static func memoryTreeResources(
        _ resources: [MemoryResource],
        activeProjectId: String?,
        selectedOrgResourceIds: Set<String>
    ) -> [MemoryResource] {
        guard let activeProjectId else {
            return resources.filter { $0.scope == .org }
        }
        let adapted = Set(resources.filter { $0.scope == .project && $0.projectId == activeProjectId }.compactMap { $0.orgSource?.resourceId })
        return resources.filter { resource in
            switch resource.scope {
            case .org:
                selectedOrgResourceIds.contains(resource.id) && !adapted.contains(resource.id)
            case .project:
                resource.projectId == activeProjectId
            }
        }
    }

    nonisolated static func memoryTreeDrafts(
        _ drafts: [LocalDraft],
        activeProjectId: String?
    ) -> [LocalDraft] {
        // LocalDraft is always a Project-bound overlay, including an Org-
        // targeted Draft. The Org catalog presents shared authority only.
        guard let activeProjectId else { return [] }
        return drafts.filter { draft in
            guard draft.hasChanges, draft.status != .discarded && draft.status != .merged else {
                return false
            }
            return draft.projectId == activeProjectId
        }
    }

    nonisolated static func preferredMemoryTreeDrafts(
        _ drafts: [LocalDraft]
    ) -> [LocalDraft] {
        var preferred: [String: LocalDraft] = [:]
        for draft in drafts {
            let target = draft.targetId ?? draft.orgSource?.resourceId ?? "draft:\(draft.id)"
            let key = "\(draft.scope.rawValue):\(target)"
            if let current = preferred[key], current.updatedAt >= draft.updatedAt {
                continue
            }
            preferred[key] = draft
        }
        return preferred.values.sorted { lhs, rhs in
            if lhs.updatedAt != rhs.updatedAt { return lhs.updatedAt > rhs.updatedAt }
            return lhs.id < rhs.id
        }
    }

    nonisolated static func hasActiveDraft(
        in projectId: String,
        targetingAny resourceIds: Set<String>,
        drafts: [LocalDraft]
    ) -> Bool {
        memoryTreeDrafts(drafts, activeProjectId: projectId).contains { draft in
            draft.targetId.map(resourceIds.contains) == true
        }
    }

    nonisolated static func memoryTabDraft(
        itemId: String,
        projectId: String?,
        drafts: [LocalDraft]
    ) -> LocalDraft? {
        guard let projectId else { return nil }
        let matchingDrafts = memoryTreeDrafts(drafts, activeProjectId: projectId).filter { draft in
            (draft.id == itemId || draft.targetId == itemId || draft.orgSource?.resourceId == itemId)
        }
        return matchingDrafts.first { $0.id == itemId }
            ?? preferredMemoryTreeDrafts(matchingDrafts).first { $0.scope == .project }
            ?? preferredMemoryTreeDrafts(matchingDrafts).first
    }

    nonisolated static func unrepresentedDrafts(
        _ activeDrafts: [LocalDraft],
        authoritativeResourceIds: Set<String>
    ) -> [LocalDraft] {
        activeDrafts.filter { draft in
            guard let targetId = draft.targetId ?? draft.orgSource?.resourceId else { return true }
            return !authoritativeResourceIds.contains(targetId)
        }
    }
}
