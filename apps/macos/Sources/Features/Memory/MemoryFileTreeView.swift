import AppKit
import SwiftUI

enum MemoryFileTreeAlert: Identifiable {
    enum ID: Hashable {
        case itemRename(String)
        case directoryRename(String)
        case memoryDeletion
        case directoryDiscard
        case directoryDeletion
    }

    case itemRename(item: MemoryListItem)
    case directoryRename(id: String, items: [MemoryListItem])
    case memoryDeletion(items: [MemoryListItem])
    case directoryDiscard(name: String, drafts: [LocalDraft])
    case directoryDeletion(name: String, plan: MemoryDirectoryDeletionPlan)

    var id: ID {
        switch self {
        case .itemRename(let item):
            .itemRename(item.id)
        case .directoryRename(let id, _):
            .directoryRename(id)
        case .memoryDeletion:
            .memoryDeletion
        case .directoryDiscard:
            .directoryDiscard
        case .directoryDeletion:
            .directoryDeletion
        }
    }

    var title: String {
        switch self {
        case .itemRename(let item):
            String(localized: "Rename \(item.document.path.split(separator: "/").last.map(String.init) ?? String(localized: "File"))")
        case .directoryRename:
            String(localized: "Rename Folder")
        case .memoryDeletion(let items):
            items.count == 1
                ? String(localized: "Delete File?")
                : String(localized: "Delete \(items.count) Files?")
        case .directoryDiscard(let name, let drafts):
            drafts.count == 1
                ? String(localized: "Discard Draft in \(name)?")
                : String(localized: "Discard \(drafts.count) Drafts in \(name)?")
        case .directoryDeletion(let name, _):
            String(localized: "Delete \(name)?")
        }
    }

    var confirmationTitle: String {
        switch self {
        case .itemRename, .directoryRename:
            String(localized: "Rename")
        case .memoryDeletion:
            String(localized: "Delete")
        case .directoryDiscard:
            String(localized: "Discard Drafts")
        case .directoryDeletion:
            String(localized: "Delete Folder")
        }
    }

    var message: String {
        switch self {
        case .itemRename(let item):
            if item.resource == nil {
                return String(localized: "This changes the path in the current Project-carried Draft.")
            }
            return String(localized: "The rename is saved as a Project draft and takes effect for project members after review and merge.")
        case .directoryRename(_, let items):
            let sharedCount = items.filter { $0.resource != nil }.count
            let draftCount = items.count - sharedCount
            if sharedCount == 0 {
                return String(localized: "This preserves every relative file path in the current Project-carried Drafts. Remote Organization Memory is unchanged.")
            }
            if draftCount > 0 {
                return String(localized: "This preserves every relative file path, renames \(draftCount) unpublished Drafts, and creates \(sharedCount) rename proposals. Shared Memory changes only after review and merge.")
            }
            return String(localized: "This preserves every relative file path and creates Project-carried rename Drafts. Shared Memory changes only after review and merge.")
        case .memoryDeletion(let items):
            if items.count == 1 {
                return String(localized: "The deletion is saved as a draft. After review and merge, this file will be deleted from its publication owner.")
            }
            return String(localized: "The deletion is saved as a draft. After review and merge, these \(items.count) files will be deleted from their publication owner.")
        case .directoryDiscard:
            return String(localized: "This removes the Project-carried Drafts in this folder. Remote Organization Memory is unchanged.")
        case .directoryDeletion(let name, let plan):
            var effects: [String] = []
            if !plan.itemsToDelete.isEmpty {
                effects.append(String(localized: "create deletion proposals for \(plan.itemsToDelete.count) remote memories"))
            }
            if !plan.draftsToDiscard.isEmpty {
                effects.append(String(localized: "discard \(plan.draftsToDiscard.count) unpublished drafts"))
            }
            let joinedEffects = ListFormatter.localizedString(byJoining: effects)
            return String(localized: "This will \(joinedEffects) in \(name). Remote memories are removed only after review and merge.",
                          comment: "The first argument is a localized list of actions, such as creating deletion proposals and discarding drafts; the second is a folder name.")
        }
    }
}

private struct PendingDirectoryReview: Identifiable {
    let id = UUID()
    let drafts: [LocalDraft]
    let initialTitle: String
}

struct FileTreeView: View {
    @EnvironmentObject private var memoryCatalog: MemoryCatalog
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var draftStore: DraftStore
    @EnvironmentObject private var workspaceFeedback: WorkspaceFeedback
    @EnvironmentObject private var memoryModel: MemoryModel
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation
    @EnvironmentObject private var projectService: ProjectService
    @EnvironmentObject private var reconciler: DraftReconciliationService
    @EnvironmentObject private var daemonSync: DaemonSyncService
    @EnvironmentObject private var reviewModel: ReviewsModel
    @EnvironmentObject private var documentSessions: DocumentSessions
    let items: [MemoryListItem]
    @State private var expandedDirectoryIds: Set<String> = []
    @State private var selectedNodeIds: Set<String> = []
    @State private var selectionAnchorId: String?
    @State private var initializedExpansion = false
    @State private var proposedName = ""
    @State private var proposedDirectoryName = ""
    @State private var pendingAlert: MemoryFileTreeAlert?
    @State private var pendingDirectoryReview: PendingDirectoryReview?
    @StateObject private var operations: MemoryFileOperationsModel

    init(items: [MemoryListItem], operations: @autoclosure @escaping () -> MemoryFileOperationsModel) {
        self.items = items
        _operations = StateObject(wrappedValue: operations())
    }

    private var roots: [FileTreeNode] {
        FileTreeNode.build(items)
    }

    private var visibleNodes: [VisibleFileTreeNode] {
        FileTreeNode.visibleNodes(roots, expandedDirectoryIds: expandedDirectoryIds)
    }

    var body: some View {
        fileTreeWithAlert
        .sheet(item: $pendingDirectoryReview) { request in
            ReviewRequestSheet(
                initialTitle: request.initialTitle,
                drafts: request.drafts,
                loadCandidates: {
                    try await self.reconciler.reconciliationCandidates(for: request.drafts)
                }
            ) { title, description, reconciliations, contributions in
                try await self.reviewModel.requestReview(
                    for: request.drafts,
                    title: title,
                    description: description,
                    reconciliations: reconciliations,
                    contributions: contributions
                )
            }
        }
    }

    private var fileTreeContent: some View {
        List(selection: selection) {
            ForEach(self.visibleNodes) { entry in
                self.fileTreeRow(for: entry)
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color(nsColor: .controlBackgroundColor))
        .pageFeedback(operations.directoryOperationProgress, isStatus: true)
        .contextMenu(forSelectionType: String.self) { nodeIds in
            self.fileTreeMenu(for: nodeIds)
        }
        .onAppear {
            guard !self.initializedExpansion else { return }
            self.expandedDirectoryIds = FileTreeNode.directoryIds(in: self.roots)
            self.initializedExpansion = true
            self.synchronizeSelectionWithActiveItem()
        }
        .onChange(of: items.map { "\($0.id):\($0.document.path)" }) { _, _ in
            self.expandedDirectoryIds.formUnion(FileTreeNode.directoryIds(in: self.roots))
            self.selectedNodeIds.formIntersection(Set(FileTreeNode.allIds(in: self.roots)))
            if let selectionAnchorId,
               FileTreeNode.node(withId: selectionAnchorId, in: roots) == nil {
                self.selectionAnchorId = nil
            }
            self.synchronizeSelectionWithActiveItem()
        }
        .onChange(of: workspaceNavigation.activeVisibleTab?.itemId ?? workspaceNavigation.selectedItemId) { _, _ in
            self.synchronizeSelectionWithActiveItem()
        }
        .onChange(of: workspaceContext.activeProjectId) { _, _ in
            self.dismissAlert()
            self.pendingDirectoryReview = nil
        }
    }

    private var fileTreeWithAlert: some View {
        fileTreeContent
        .alert(
            pendingAlert?.title ?? "",
            isPresented: Binding(
                get: { self.pendingAlert != nil },
                set: { if !$0 { self.dismissAlert() } }
            ),
            presenting: pendingAlert
        ) { alert in
            switch alert {
            case .itemRename:
                TextField("File name", text: self.$proposedName)
                Button("Cancel", role: .cancel) { self.dismissAlert() }
                Button(alert.confirmationTitle) { self.renameSelectedItem() }
                    .disabled(
                        self.operations.directoryOperationProgress != nil || !self.isValidProposedName
                    )
            case .directoryRename:
                TextField("Folder name", text: self.$proposedDirectoryName)
                Button("Cancel", role: .cancel) { self.dismissAlert() }
                Button(alert.confirmationTitle) { self.renameSelectedDirectory() }
                    .disabled(
                        self.operations.directoryOperationProgress != nil || !self.isValidProposedDirectoryName
                    )
            case .memoryDeletion, .directoryDiscard, .directoryDeletion:
                Button("Cancel", role: .cancel) { self.dismissAlert() }
                Button(alert.confirmationTitle, role: .destructive) {
                    self.confirm(alert)
                }
                .disabled(self.operations.directoryOperationProgress != nil)
            }
        } message: { alert in
            Text(alert.message)
        }
    }

    private func fileTreeRow(for entry: VisibleFileTreeNode) -> some View {
        return FileTreeRow(
            entry: entry,
            isExpanded: expandedDirectoryIds.contains(entry.id),
            onDirectoryClick: { modifierFlags in
                self.handleDirectoryClick(entry.id, modifierFlags: modifierFlags)
            }
        )
        .tag(entry.id)
        .listRowInsets(.init(top: 0, leading: 5, bottom: 0, trailing: 5))
        .listRowSeparator(.hidden)
    }

    private var isValidProposedName: Bool {
        let name = proposedName.trimmingCharacters(in: .whitespacesAndNewlines)
        return !name.isEmpty && name != "." && name != ".." && !name.contains("/")
    }

    private var itemToRename: MemoryListItem? {
        guard case .itemRename(let item) = pendingAlert else { return nil }
        return item
    }

    private var directoryToRenameId: String? {
        guard case .directoryRename(let id, _) = pendingAlert else { return nil }
        return id
    }

    private var isValidProposedDirectoryName: Bool {
        let name = proposedDirectoryName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name != ".", name != "..", !name.contains("/"),
              let directoryToRenameId,
              let directory = FileTreeNode.node(withId: directoryToRenameId, in: roots) else {
            return false
        }
        return name != directory.name
    }

    private func dismissAlert() {
        pendingAlert = nil
        proposedName = ""
        proposedDirectoryName = ""
    }

    private func beginRenaming(_ item: MemoryListItem) {
        guard operations.directoryOperationProgress == nil else { return }
        guard !documentSessions.isSynchronizingDocument(item.id) else {
            workspaceFeedback.errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
            return
        }
        proposedName = item.document.path.split(separator: "/").last.map(String.init)
            ?? item.document.path
        pendingAlert = .itemRename(item: item)
    }

    private func renameSelectedItem() {
        guard operations.directoryOperationProgress == nil else { return }
        guard let item = itemToRename else { return }
        guard !documentSessions.isSynchronizingDocument(item.id) else {
            dismissAlert()
            workspaceFeedback.errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
            return
        }
        let name = proposedName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isValidProposedName else { return }
        var document = item.document
        let parent = document.path
            .split(separator: "/")
            .dropLast()
            .joined(separator: "/")
        document.path = parent.isEmpty ? name : "\(parent)/\(name)"
        dismissAlert()
        Task {
            do {
                try await self.draftStore.rename(item, to: document.path)
            } catch {
                self.workspaceFeedback.errorMessage = error.actionMessage
            }
        }
    }

    private func beginRenamingDirectory(_ directory: FileTreeNode) {
        guard operations.directoryOperationProgress == nil else { return }
        let targetItems = FileTreeNode.items(
            in: roots,
            selectedNodeIds: [directory.id]
        )
        guard targetItems.allSatisfy({
            MemoryFileTreeMenu.canRename($0, inOrgView: false)
                && self.draftStore.canEditMemory($0)
                && !self.documentSessions.isSynchronizingDocument($0.id)
        }) else {
            workspaceFeedback.errorMessage = MemoryDirectoryMutationError.readOnly.localizedDescription
            return
        }
        proposedDirectoryName = directory.name
        pendingAlert = .directoryRename(id: directory.id, items: targetItems)
    }

    private func renameSelectedDirectory() {
        guard operations.directoryOperationProgress == nil else { return }
        guard let directoryToRenameId else { return }
        let name = proposedDirectoryName.trimmingCharacters(in: .whitespacesAndNewlines)
        let targetItems = FileTreeNode.items(
            in: roots,
            selectedNodeIds: [directoryToRenameId]
        )
        do {
            guard targetItems.allSatisfy({
                MemoryFileTreeMenu.canRename($0, inOrgView: false)
                    && self.draftStore.canEditMemory($0)
                    && !self.documentSessions.isSynchronizingDocument($0.id)
            }) else {
                throw MemoryDirectoryMutationError.readOnly
            }
            let resources = memoryCatalog.resources.filter {
                $0.scope == .org
                    || ($0.scope == .project && $0.projectId == self.workspaceContext.activeProjectId)
            }
            let drafts = draftStore.drafts.filter {
                $0.projectId == self.workspaceContext.activeProjectId
                    && $0.status != .discarded
                    && $0.status != .merged
            }
            let occupiedPaths = Set(resources.map(\.document.path))
                .union(drafts.map(\.document.path))
            let occupiedTreePaths = Set(items.map { FileTreeNode.treePath(for: $0) })
            let plan = try MemoryFileTreeMenu.directoryRenamePlan(
                directoryId: directoryToRenameId,
                newName: name,
                items: targetItems,
                occupiedPaths: occupiedPaths,
                occupiedTreePaths: occupiedTreePaths,
                inOrgView: false
            )
            dismissAlert()
            Task { await self.operations.renameDirectory(plan) }
        } catch {
            workspaceFeedback.errorMessage = error.actionMessage
        }
    }

    private var selection: Binding<Set<String>> {
        Binding(
            get: { self.selectedNodeIds },
            set: { newSelection in
                let previous = self.selectedNodeIds
                self.selectedNodeIds = newSelection
                guard newSelection.count == 1,
                      let nodeId = newSelection.first,
                      let node = FileTreeNode.node(withId: nodeId, in: roots) else {
                    return
                }

                if newSelection != previous {
                    self.selectionAnchorId = nodeId
                }
                guard newSelection != previous, let item = node.item else { return }
                self.workspaceNavigation.open(item)
            }
        )
    }

    private func handleDirectoryClick(
        _ nodeId: String,
        modifierFlags: NSEvent.ModifierFlags
    ) {
        let result = FileTreeSelectionInteraction.directoryClick(
            nodeId: nodeId,
            visibleNodeIds: visibleNodes.map(\.id),
            currentSelection: selectedNodeIds,
            anchorId: selectionAnchorId,
            modifierFlags: modifierFlags
        )
        selectedNodeIds = result.selection
        selectionAnchorId = result.anchorId
        if result.togglesDirectory {
            toggleDirectory(nodeId)
        }
    }

    private func toggleDirectory(_ nodeId: String) {
        withAnimation(.snappy(duration: 0.14)) {
            if self.expandedDirectoryIds.contains(nodeId) {
                self.expandedDirectoryIds.remove(nodeId)
            } else {
                self.expandedDirectoryIds.insert(nodeId)
            }
        }
    }

    @ViewBuilder
    private func fileTreeMenu(for nodeIds: Set<String>) -> some View {
        let targetItems = FileTreeNode.items(in: roots, selectedNodeIds: nodeIds)
        let exportItems = FileTreeNode.items(
            in: FileTreeNode.build(memoryModel.visibleMemoryItems),
            selectedNodeIds: nodeIds
        )
        let selectedDirectory = FileTreeNode.selectedDirectory(
            in: roots,
            selectedNodeIds: nodeIds
        )
        let singleItem = selectedDirectory == nil && targetItems.count == 1
            ? targetItems.first
            : nil
        let isOrgView = workspaceContext.activeProjectId == nil
        let addableItems = MemoryFileTreeMenu.addable(targetItems, inOrgView: isOrgView)
        let removableItems = MemoryFileTreeMenu.removable(targetItems, inOrgView: isOrgView)
        let trashableItems = MemoryFileTreeMenu.trashable(targetItems, inOrgView: isOrgView)
            .filter { self.draftStore.canEditMemory($0) }
        let singleRenameable = singleItem.map {
            MemoryFileTreeMenu.canRename($0, inOrgView: isOrgView)
                && self.draftStore.canEditMemory($0)
        } ?? false
        let singleTrashable = singleItem.map { item in
            trashableItems.contains { $0.id == item.id }
        } ?? false
        let singleStale = singleItem.map { item in
            item.resource.map { self.memoryCatalog.staleResourceIds.contains($0.id) } == true
        } ?? false
        let singleSynchronizing = singleItem.map {
            self.documentSessions.isSynchronizingDocument($0.id)
        } ?? false
        let selectionContainsSynchronizingDocument = targetItems.contains {
            self.documentSessions.isSynchronizingDocument($0.id)
        }
        let trashSelectionContainsSynchronizingDocument = trashableItems.contains {
            self.documentSessions.isSynchronizingDocument($0.id)
        }
        let reviewDrafts = MemoryFileTreeMenu.reviewableDrafts(
            targetItems,
            inOrgView: isOrgView
        )
        let reviewSelectionIsReady = MemoryFileTreeMenu.isReviewSelectionReady(reviewDrafts)
        let directoryDrafts = selectedDirectory == nil
            ? []
            : MemoryFileTreeMenu.discardableDrafts(targetItems, inOrgView: isOrgView)
        let directoryRenameable = selectedDirectory != nil
            && !targetItems.isEmpty
            && targetItems.allSatisfy {
                MemoryFileTreeMenu.canRename($0, inOrgView: isOrgView)
                    && self.draftStore.canEditMemory($0)
            }
        let directoryDeletionPlan = selectedDirectory.flatMap { _ in
            MemoryFileTreeMenu.directoryDeletionPlan(targetItems, inOrgView: isOrgView)
        }
        let directoryDeletionAllowed = directoryDeletionPlan?.itemsToDelete.allSatisfy {
            self.draftStore.canEditMemory($0)
        } == true
        let hasDraftAction = !isOrgView
            && (singleItem?.draft != nil || !directoryDrafts.isEmpty)
        let hasDomainSection = !addableItems.isEmpty || !removableItems.isEmpty
            || hasDraftAction || !reviewDrafts.isEmpty || singleStale

        // ---- generic document operations (standard macOS conventions) ----
        if let selectedDirectory {
            if directoryRenameable {
                Button("Rename Folder…") { self.beginRenamingDirectory(selectedDirectory) }
                    .disabled(
                        operations.directoryOperationProgress != nil
                            || selectionContainsSynchronizingDocument
                    )
            }
            if let directoryDeletionPlan, directoryDeletionAllowed {
                Button("Delete Folder…", role: .destructive) {
                    self.pendingAlert = .directoryDeletion(
                        name: selectedDirectory.name,
                        plan: directoryDeletionPlan
                    )
                }
                .disabled(
                    operations.directoryOperationProgress != nil
                        || selectionContainsSynchronizingDocument
                )
            }
        } else if let singleItem {
            Button("Open") { self.workspaceNavigation.open(singleItem) }
            if singleItem.supportsMarkdownPreview {
                Button("Open Source") { self.workspaceNavigation.open(singleItem, mode: .source) }
            }
            if !isOrgView, singleItem.resource?.scope == .org {
                Button("Propose Organization Change…") {
                    Task { await self.memoryModel.proposeOrganizationChange(singleItem) }
                }
                .disabled(singleSynchronizing)
            }
            if singleRenameable {
                Button("Rename…") { self.beginRenaming(singleItem) }
                    .disabled(operations.directoryOperationProgress != nil || singleSynchronizing)
            }
            if singleTrashable {
                Button("Delete…", role: .destructive) {
                    self.proposeMemoryDeletion([singleItem])
                }
                .disabled(operations.directoryOperationProgress != nil || singleSynchronizing)
            }
        } else if !targetItems.isEmpty {
            Button("Open") { targetItems.forEach { self.workspaceNavigation.open($0) } }
            if !trashableItems.isEmpty {
                Button(memoryDeletionTitle(count: trashableItems.count), role: .destructive) {
                    self.proposeMemoryDeletion(trashableItems)
                }
                .disabled(
                    operations.directoryOperationProgress != nil
                        || trashSelectionContainsSynchronizingDocument
                )
            }
        }

        if !exportItems.isEmpty {
            Button("Export as ZIP…") {
                self.memoryModel.exportMemory(
                    exportItems,
                    name: selectedDirectory?.name ?? singleItem?.document.title
                )
            }
            .disabled(operations.directoryOperationProgress != nil || !memoryModel.canExportMemory(exportItems))
        }

        // ---- domain operations (Memory scope relationships and drafts) ----
        if hasDomainSection {
            Divider()
        }
        if !addableItems.isEmpty {
            Menu(addToProjectTitle(count: addableItems.count)) {
                if self.workspaceContext.projects.isEmpty {
                    Button("No Projects") {}
                        .disabled(true)
                } else {
                    ForEach(self.workspaceContext.projects) { project in
                        Button("Add to \(project.name)") {
                            Task { await self.operations.addToProject(addableItems, projectId: project.id) }
                        }
                        .disabled(!self.workspaceContext.canManageProject(project.id))
                    }
                }
            }
            .disabled(
                operations.directoryOperationProgress != nil
                    || !workspaceContext.projects.contains(where: { self.workspaceContext.canManageProject($0.id) })
                    || workspaceContext.projects.isEmpty
                    || selectionContainsSynchronizingDocument
            )
        }
        if !removableItems.isEmpty {
            Button(removeFromProjectTitle(count: removableItems.count)) {
                Task { await self.operations.removeFromProject(removableItems) }
            }
            .disabled(
                operations.directoryOperationProgress != nil
                    || workspaceContext.activeProjectId.map { !self.workspaceContext.canManageProject($0) } != false
                    || selectionContainsSynchronizingDocument
            )
            .help("Remove the reference from this project. The remote file is kept.")
        }
        if !isOrgView, !reviewDrafts.isEmpty {
            Button(reviewRequestTitle(count: reviewDrafts.count)) {
                self.pendingDirectoryReview = .init(
                    drafts: reviewDrafts,
                    initialTitle: self.directoryReviewTitle(for: nodeIds, draftCount: reviewDrafts.count)
                )
            }
            .disabled(
                operations.directoryOperationProgress != nil
                    || !reviewSelectionIsReady
                    || selectionContainsSynchronizingDocument
            )
        }
        if !isOrgView, let draft = singleItem?.draft, draft.status == .submitted {
            Button("View Review") {
                Task { await self.reviewModel.openReview(for: draft) }
            }
        }
        if let selectedDirectory, !directoryDrafts.isEmpty {
            Button(
                directoryDrafts.count == 1
                    ? "Discard Draft in Folder…"
                    : "Discard \(directoryDrafts.count) Drafts in Folder…",
                role: .destructive
            ) {
                self.pendingAlert = .directoryDiscard(
                    name: selectedDirectory.name,
                    drafts: directoryDrafts
                )
            }
            .disabled(
                operations.directoryOperationProgress != nil
                    || selectionContainsSynchronizingDocument
            )
        }
        if let singleItem {
            let resourceIsStale = singleItem.resource.map {
                self.memoryCatalog.staleResourceIds.contains($0.id)
            } == true
            if documentSessions.isSynchronizingDocument(singleItem.id) {
                Button("Checking Remote Changes…") {}
                    .disabled(true)
            } else if let draft = singleItem.draft,
                      draft.freshness == .behind || resourceIsStale {
                switch draft.syncStatus {
                case .queued, .syncing, .retrying:
                    Button("Uploading Draft Changes…") {}
                        .disabled(true)
                case .failed:
                    if daemonSync.isRetryingSync(
                        channel: "drafts",
                        projectId: draft.projectId
                    ) {
                        Button("Retrying Draft Sync…") {}
                            .disabled(true)
                    } else {
                        Button("Retry Draft Sync") {
                            Task {
                                _ = await self.daemonSync.retrySync(
                                    channel: "drafts",
                                    projectId: draft.projectId
                                )
                            }
                        }
                        .disabled(operations.directoryOperationProgress != nil)
                    }
                case .synced:
                    if draft.serverId == nil {
                        Button("Draft Not Ready") {}
                            .disabled(true)
                    } else {
                        Button(
                            draft.hasUpstreamResourceChanges
                                ? "Review Remote Changes"
                                : "Update from Remote Version"
                        ) {
                            guard self.operations.directoryOperationProgress == nil else { return }
                            self.memoryModel.syncDocument(singleItem)
                        }
                        .disabled(operations.directoryOperationProgress != nil)
                    }
                }
            } else if resourceIsStale {
                Button("Update from Remote Version") {
                    guard self.operations.directoryOperationProgress == nil else { return }
                    self.memoryModel.syncDocument(singleItem)
                }
                .disabled(operations.directoryOperationProgress != nil)
            }
            if !isOrgView, let draft = singleItem.draft {
                Button("Discard Draft") {
                    guard self.operations.directoryOperationProgress == nil else { return }
                    Task { await self.draftStore.discard(draft) }
                }
                .disabled(
                    operations.directoryOperationProgress != nil
                        || singleSynchronizing
                )
            }
        }

        if targetItems.isEmpty {
            if let scope = MemoryFileTreeMenu.creationScope(inOrgView: isOrgView) {
                Button("Propose New Project Memory") {
                    guard self.operations.directoryOperationProgress == nil else { return }
                    Task {
                        await self.memoryModel.createMemory(kind: self.workspaceNavigation.selectedKind, scope: scope)
                    }
                }
                .disabled(
                    operations.directoryOperationProgress != nil
                        || !draftStore.canCreateMemory(kind: workspaceNavigation.selectedKind, scope: scope)
                )
            }
        }
    }

    private func synchronizeSelectionWithActiveItem() {
        guard selectedNodeIds.count <= 1,
              let itemId = workspaceNavigation.activeVisibleTab?.itemId ?? workspaceNavigation.selectedItemId,
              FileTreeNode.node(withId: itemId, in: roots) != nil else {
            return
        }
        selectedNodeIds = [itemId]
        selectionAnchorId = itemId
    }

    private func memoryDeletionTitle(count: Int) -> String {
        count == 1 ? String(localized: "Delete…") : String(localized: "Delete \(count) Files…")
    }

    private func reviewRequestTitle(count: Int) -> String {
        count == 1 ? String(localized: "Request Review…") : String(localized: "Request Review for \(count) Changes…")
    }

    private func directoryReviewTitle(for nodeIds: Set<String>, draftCount: Int) -> String {
        if nodeIds.count == 1,
           let nodeId = nodeIds.first,
           let node = FileTreeNode.node(withId: nodeId, in: roots),
           node.item == nil {
            return String(localized: "Update \(node.name)")
        }
        return draftCount == 1 ? String(localized: "Update memory") : String(localized: "Update \(draftCount) memories")
    }

    private func proposeMemoryDeletion(_ items: [MemoryListItem]) {
        guard !items.isEmpty else { return }
        pendingAlert = .memoryDeletion(items: items)
    }

    private func confirm(_ alert: MemoryFileTreeAlert) {
        dismissAlert()
        switch alert {
        case .itemRename, .directoryRename:
            return
        case .memoryDeletion(let items):
            Task { await self.operations.deleteItems(items) }
        case .directoryDiscard(_, let drafts):
            Task { await self.operations.discardDrafts(drafts) }
        case .directoryDeletion(_, let plan):
            Task { await self.operations.deleteDirectory(plan) }
        }
    }

    private func addToProjectTitle(count: Int) -> String {
        count == 1 ? String(localized: "Add to Project") : String(localized: "Add \(count) Items to Project")
    }

    private func removeFromProjectTitle(count: Int) -> String {
        count == 1 ? String(localized: "Remove from Project") : String(localized: "Remove \(count) Items from Project")
    }
}

/// Git-style title color for a memory file-tree row.
enum MemoryFileTreeTitleTone: Equatable {
    case primary
    case newDraft
    case modifiedDraft
    case deletedDraft

    static func resolve(item: MemoryListItem?) -> Self {
        guard let item else { return .primary }
        guard let draft = item.draft else { return .primary }
        guard draft.status == .open || draft.status == .submitted else { return .primary }
        if draft.isDeletion { return .deletedDraft }
        if draft.targetId == nil { return .newDraft }
        return .modifiedDraft
    }

    var color: Color {
        switch self {
        case .primary: return .primary
        case .newDraft: return .green
        case .modifiedDraft: return Color(red: 0.8, green: 0.6, blue: 0.1) // amber, legible in light mode
        case .deletedDraft: return .red
        }
    }
}

private struct FileTreeRow: View {
    let entry: VisibleFileTreeNode
    let isExpanded: Bool
    let onDirectoryClick: (NSEvent.ModifierFlags) -> Void

    private var item: MemoryListItem? { entry.node.item }

    @ViewBuilder
    var body: some View {
        if item == nil {
            rowContent
                .simultaneousGesture(
                    TapGesture().onEnded {
                        self.onDirectoryClick(NSEvent.modifierFlags)
                    }
                )
        } else {
            rowContent
        }
    }

    private var rowContent: some View {
        PathTreeRowLabel(
            name: entry.node.name,
            path: item?.document.path,
            depth: entry.depth,
            isDirectory: item == nil,
            isExpanded: isExpanded,
            titleColor: MemoryFileTreeTitleTone.resolve(item: item).color
        ) {
            EmptyView()
        }
        .help(entry.node.name)
    }
}
