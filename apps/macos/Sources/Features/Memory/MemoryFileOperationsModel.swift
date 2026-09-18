import Combine
import Foundation

@MainActor
final class MemoryFileOperationsModel: ObservableObject {
    private let workspaceContext: WorkspaceContext
    private let workspaceFeedback: WorkspaceFeedback
    private let draftStore: DraftStore
    private let projectService: ProjectService
    private let rename: (MemoryListItem, String) async throws -> Void
    @Published private(set) var directoryOperationProgress: String?

    init(context: WorkspaceContext, feedback: WorkspaceFeedback, drafts: DraftStore, projects: ProjectService,
         rename: ((MemoryListItem, String) async throws -> Void)? = nil) {
        workspaceContext = context
        workspaceFeedback = feedback
        draftStore = drafts
        projectService = projects
        self.rename = rename ?? { try await drafts.rename($0, to: $1) }
    }

    func deleteItems(_ items: [MemoryListItem]) async {
        guard directoryOperationProgress == nil else { return }
        let authority = workspaceContext.authorityGeneration
        let project = workspaceContext.activeProjectId
        directoryOperationProgress = "Proposing \(items.count) deletions…"
        defer { directoryOperationProgress = nil }
        for (index, item) in items.enumerated() {
            guard isCurrent(authority: authority, project: project) else { return }
            directoryOperationProgress =
                "Proposing deletion \(index + 1) of \(items.count)…"
            guard await draftStore.delete(item) else { return }
        }
    }

    func discardDrafts(_ drafts: [LocalDraft]) async {
        guard directoryOperationProgress == nil else { return }
        let authority = workspaceContext.authorityGeneration
        let project = workspaceContext.activeProjectId
        directoryOperationProgress = "Discarding \(drafts.count) Drafts…"
        defer { directoryOperationProgress = nil }
        for (index, draft) in drafts.enumerated() {
            guard isCurrent(authority: authority, project: project) else { return }
            directoryOperationProgress =
                "Discarding Draft \(index + 1) of \(drafts.count)…"
            guard await draftStore.discard(draft) else {
                guard isCurrent(authority: authority, project: project) else { return }
                let detail = workspaceFeedback.errorMessage ?? "The remaining Drafts were not changed."
                workspaceFeedback.errorMessage = "Discarded \(index) of \(drafts.count) Drafts. " + detail
                return
            }
        }
    }

    func deleteDirectory(_ plan: MemoryDirectoryDeletionPlan) async {
        guard directoryOperationProgress == nil else { return }
        let authority = workspaceContext.authorityGeneration
        let project = workspaceContext.activeProjectId
        let total = plan.itemsToDelete.count + plan.draftsToDiscard.count
        var completed = 0
        directoryOperationProgress = "Applying \(total) folder changes…"
        defer { directoryOperationProgress = nil }
        for item in plan.itemsToDelete {
            guard isCurrent(authority: authority, project: project) else { return }
            directoryOperationProgress =
                "Applying folder change \(completed + 1) of \(total)…"
            guard await draftStore.delete(item) else {
                guard isCurrent(authority: authority, project: project) else { return }
                let detail = workspaceFeedback.errorMessage ?? "The remaining files were not changed."
                workspaceFeedback.errorMessage =
                    "Completed \(completed) of \(total) folder changes. " + detail
                return
            }
            completed += 1
        }
        for draft in plan.draftsToDiscard {
            guard isCurrent(authority: authority, project: project) else { return }
            directoryOperationProgress =
                "Applying folder change \(completed + 1) of \(total)…"
            guard await draftStore.discard(draft) else {
                guard isCurrent(authority: authority, project: project) else { return }
                let detail = workspaceFeedback.errorMessage ?? "The remaining files were not changed."
                workspaceFeedback.errorMessage =
                    "Completed \(completed) of \(total) folder changes. " + detail
                return
            }
            completed += 1
        }
    }

    func addToProject(_ items: [MemoryListItem], projectId: String) async {
        guard directoryOperationProgress == nil else { return }
        let authority = workspaceContext.authorityGeneration
        let project = workspaceContext.activeProjectId
        directoryOperationProgress = "Updating project memory…"
        defer { directoryOperationProgress = nil }
        do {
            try await projectService.addOrgMemories(
                resourceIds: Set(items.map(\.id)),
                toProject: projectId
            )
        } catch {
            guard isCurrent(authority: authority, project: project) else { return }
            workspaceFeedback.errorMessage = error.localizedDescription
        }
    }

    func removeFromProject(_ items: [MemoryListItem]) async {
        guard directoryOperationProgress == nil else { return }
        let authority = workspaceContext.authorityGeneration
        let project = workspaceContext.activeProjectId
        guard let projectId = workspaceContext.activeProjectId else { return }
        directoryOperationProgress = "Updating project memory…"
        defer { directoryOperationProgress = nil }
        do {
            try await projectService.removeOrgMemories(
                resourceIds: Set(items.map(\.id)),
                fromProject: projectId
            )
        } catch {
            guard isCurrent(authority: authority, project: project) else { return }
            workspaceFeedback.errorMessage = error.localizedDescription
        }
    }

    func renameDirectory(_ plan: MemoryDirectoryRenamePlan) async {
        guard directoryOperationProgress == nil else { return }
        let authority = workspaceContext.authorityGeneration
        let project = workspaceContext.activeProjectId
        var completed = 0
        directoryOperationProgress = "Renaming \(plan.changes.count) memories…"
        defer { directoryOperationProgress = nil }
        do {
            for (index, change) in plan.changes.enumerated() {
                guard isCurrent(authority: authority, project: project) else { return }
                directoryOperationProgress =
                    "Renaming \(index + 1) of \(plan.changes.count) memories…"
                try await rename(change.item, change.newPath)
                completed += 1
            }
        } catch {
            guard isCurrent(authority: authority, project: project) else { return }
            let prefix = completed == 0
                ? ""
                : "Renamed \(completed) of \(plan.changes.count) memories. "
            workspaceFeedback.errorMessage = prefix + error.localizedDescription
        }
    }

    private func isCurrent(authority: UUID, project: String?) -> Bool {
        !Task.isCancelled && workspaceContext.authorityGeneration == authority
            && workspaceContext.activeProjectId == project && !workspaceContext.isSwitchingMemoryContext
    }

}
