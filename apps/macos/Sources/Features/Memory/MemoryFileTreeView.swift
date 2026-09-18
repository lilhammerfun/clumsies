import AppKit
import SwiftUI

enum MemoryFileTreeAlert: Identifiable {
    enum ID: Hashable {
        case itemRename(String)
        case directoryRename(String)
        case organizationDeletion
        case directoryDiscard
        case directoryDeletion
    }

    case itemRename(item: MemoryListItem)
    case directoryRename(id: String, items: [MemoryListItem])
    case organizationDeletion(items: [MemoryListItem])
    case directoryDiscard(name: String, drafts: [LocalDraft])
    case directoryDeletion(name: String, plan: MemoryDirectoryDeletionPlan)

    var id: ID {
        switch self {
        case .itemRename(let item):
            .itemRename(item.id)
        case .directoryRename(let id, _):
            .directoryRename(id)
        case .organizationDeletion:
            .organizationDeletion
        case .directoryDiscard:
            .directoryDiscard
        case .directoryDeletion:
            .directoryDeletion
        }
    }

    var title: String {
        switch self {
        case .itemRename(let item):
            "Rename \(item.document.path.split(separator: "/").last ?? "File")"
        case .directoryRename:
            "Rename Folder"
        case .organizationDeletion(let items):
            items.count == 1
                ? "Delete File?"
                : "Delete \(items.count) Files?"
        case .directoryDiscard(let name, let drafts):
            drafts.count == 1
                ? "Discard Draft in \(name)?"
                : "Discard \(drafts.count) Drafts in \(name)?"
        case .directoryDeletion(let name, _):
            "Delete \(name)?"
        }
    }

    var confirmationTitle: String {
        switch self {
        case .itemRename, .directoryRename:
            "Rename"
        case .organizationDeletion:
            "Delete"
        case .directoryDiscard:
            "Discard Drafts"
        case .directoryDeletion:
            "Delete Folder"
        }
    }

    var message: String {
        switch self {
        case .itemRename(let item):
            if item.resource == nil {
                return "This changes the path in the current Project-carried Draft."
            }
            return "The rename is saved as a draft. After review and merge, "
                + "the file will be renamed in every project that includes it."
        case .directoryRename(_, let items):
            let sharedCount = items.filter { $0.resource != nil }.count
            let draftCount = items.count - sharedCount
            if sharedCount == 0 {
                return "This preserves every relative file path in the current Project-carried "
                    + "Drafts. Shared Organization Memory is unchanged."
            }
            if draftCount > 0 {
                return "This preserves every relative file path, renames \(draftCount) unpublished "
                    + "Drafts, and creates \(sharedCount) rename proposals. Shared Organization "
                    + "Memory changes only after review and merge."
            }
            return "This preserves every relative file path and creates Project-carried rename "
                + "Drafts. Organization Memory changes only after review and merge."
        case .organizationDeletion(let items):
            let subject = items.count == 1
                ? "this organization memory"
                : "these \(items.count) organization memories"
            let proposal = items.count == 1
                ? "a deletion draft proposal"
                : "deletion draft proposals"
            let object = items.count == 1 ? "it" : "them"
            return "This creates \(proposal). If reviewed and merged, \(subject) "
                + "will be removed for every project that includes \(object)."
        case .directoryDiscard:
            return "This removes the Project-carried Drafts in this folder. "
                + "Shared Organization Memory is unchanged."
        case .directoryDeletion(let name, let plan):
            var effects: [String] = []
            if !plan.itemsToDelete.isEmpty {
                let noun = plan.itemsToDelete.count == 1 ? "memory" : "memories"
                effects.append(
                    "create deletion proposals for \(plan.itemsToDelete.count) shared "
                        + noun
                )
            }
            if !plan.draftsToDiscard.isEmpty {
                let noun = plan.draftsToDiscard.count == 1 ? "draft" : "drafts"
                effects.append(
                    "discard \(plan.draftsToDiscard.count) unpublished "
                        + noun
                )
            }
            let joinedEffects = effects.joined(separator: " and ")
            return "This will \(joinedEffects) in \(name). "
                + "Shared memories are removed only after review and merge."
        }
    }
}

private struct PendingDirectoryReview: Identifiable {
    let id = UUID()
    let drafts: [LocalDraft]
    let initialTitle: String
}

struct FileTreeView: View {
    @ObservedObject var store: WorkspaceStore
    let items: [MemoryListItem]
    @State private var expandedDirectoryIds: Set<String> = []
    @State private var selectedNodeIds: Set<String> = []
    @State private var selectionAnchorId: String?
    @State private var initializedExpansion = false
    @State private var proposedName = ""
    @State private var proposedDirectoryName = ""
    @State private var pendingAlert: MemoryFileTreeAlert?
    @State private var pendingDirectoryReview: PendingDirectoryReview?
    @State private var directoryOperationProgress: String?

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
                loadCandidates: {
                    try await store.reconciliationCandidates(for: request.drafts)
                }
            ) { title, description, reconciliations in
                try await store.requestReview(
                    for: request.drafts,
                    title: title,
                    description: description,
                    reconciliations: reconciliations
                )
            }
        }
    }

    private var fileTreeContent: some View {
        List(selection: selection) {
            ForEach(visibleNodes) { entry in
                fileTreeRow(for: entry)
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color(nsColor: .controlBackgroundColor))
        .safeAreaInset(edge: .bottom) {
            if let directoryOperationProgress {
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text(directoryOperationProgress)
                        .font(.caption)
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.bar)
                .accessibilityElement(children: .combine)
                .accessibilityLabel(directoryOperationProgress)
            }
        }
        .contextMenu(forSelectionType: String.self) { nodeIds in
            fileTreeMenu(for: nodeIds)
        }
        .onAppear {
            guard !initializedExpansion else { return }
            expandedDirectoryIds = FileTreeNode.directoryIds(in: roots)
            initializedExpansion = true
            synchronizeSelectionWithActiveItem()
        }
        .onChange(of: items.map { "\($0.id):\($0.document.path)" }) { _, _ in
            expandedDirectoryIds.formUnion(FileTreeNode.directoryIds(in: roots))
            selectedNodeIds.formIntersection(Set(FileTreeNode.allIds(in: roots)))
            if let selectionAnchorId,
               FileTreeNode.node(withId: selectionAnchorId, in: roots) == nil {
                self.selectionAnchorId = nil
            }
            synchronizeSelectionWithActiveItem()
        }
        .onChange(of: store.activeVisibleTab?.itemId ?? store.selectedItemId) { _, _ in
            synchronizeSelectionWithActiveItem()
        }
        .onChange(of: store.activeProjectId) { _, _ in
            dismissAlert()
            pendingDirectoryReview = nil
        }
    }

    private var fileTreeWithAlert: some View {
        fileTreeContent
        .alert(
            pendingAlert?.title ?? "",
            isPresented: Binding(
                get: { pendingAlert != nil },
                set: { if !$0 { dismissAlert() } }
            ),
            presenting: pendingAlert
        ) { alert in
            switch alert {
            case .itemRename:
                TextField("File name", text: $proposedName)
                Button("Cancel", role: .cancel) { dismissAlert() }
                Button(alert.confirmationTitle) { renameSelectedItem() }
                    .disabled(
                        directoryOperationProgress != nil || !isValidProposedName
                    )
            case .directoryRename:
                TextField("Folder name", text: $proposedDirectoryName)
                Button("Cancel", role: .cancel) { dismissAlert() }
                Button(alert.confirmationTitle) { renameSelectedDirectory() }
                    .disabled(
                        directoryOperationProgress != nil || !isValidProposedDirectoryName
                    )
            case .organizationDeletion, .directoryDiscard, .directoryDeletion:
                Button("Cancel", role: .cancel) { dismissAlert() }
                Button(alert.confirmationTitle, role: .destructive) {
                    confirm(alert)
                }
                .disabled(directoryOperationProgress != nil)
            }
        } message: { alert in
            Text(alert.message)
        }
    }

    private func fileTreeRow(for entry: VisibleFileTreeNode) -> some View {
        let review = entry.node.item?.draft.flatMap { store.review(for: $0) }
        return FileTreeRow(
            entry: entry,
            isExpanded: expandedDirectoryIds.contains(entry.id),
            isStale: resourceIsStale(for: entry.node.item),
            review: review,
            onOpenReview: {
                if let draft = entry.node.item?.draft {
                    Task { await store.openReview(for: draft) }
                }
            },
            onDirectoryClick: { modifierFlags in
                handleDirectoryClick(entry.id, modifierFlags: modifierFlags)
            }
        )
        .tag(entry.id)
        .listRowInsets(.init(top: 0, leading: 5, bottom: 0, trailing: 5))
        .listRowSeparator(.hidden)
    }

    private func resourceIsStale(for item: MemoryListItem?) -> Bool {
        guard let item, item.draft == nil, let resource = item.resource else { return false }
        return store.staleResourceIds.contains(resource.id)
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
        guard directoryOperationProgress == nil else { return }
        guard !store.isSynchronizingDocument(item.id) else {
            store.errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
            return
        }
        proposedName = item.document.path.split(separator: "/").last.map(String.init)
            ?? item.document.path
        pendingAlert = .itemRename(item: item)
    }

    private func renameSelectedItem() {
        guard directoryOperationProgress == nil else { return }
        guard let item = itemToRename else { return }
        guard !store.isSynchronizingDocument(item.id) else {
            dismissAlert()
            store.errorMessage = DocumentSyncError.mutationWhileSynchronizing.localizedDescription
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
                try await store.rename(item, to: document.path)
            } catch {
                store.errorMessage = error.localizedDescription
            }
        }
    }

    private func beginRenamingDirectory(_ directory: FileTreeNode) {
        guard directoryOperationProgress == nil else { return }
        let targetItems = FileTreeNode.items(
            in: roots,
            selectedNodeIds: [directory.id]
        )
        guard targetItems.allSatisfy({
            MemoryFileTreeMenu.canRename($0, inOrgView: false)
                && store.canEditMemory($0)
                && !store.isSynchronizingDocument($0.id)
        }) else {
            store.errorMessage = MemoryDirectoryMutationError.readOnly.localizedDescription
            return
        }
        proposedDirectoryName = directory.name
        pendingAlert = .directoryRename(id: directory.id, items: targetItems)
    }

    private func renameSelectedDirectory() {
        guard directoryOperationProgress == nil else { return }
        guard let directoryToRenameId else { return }
        let name = proposedDirectoryName.trimmingCharacters(in: .whitespacesAndNewlines)
        let targetItems = FileTreeNode.items(
            in: roots,
            selectedNodeIds: [directoryToRenameId]
        )
        do {
            guard targetItems.allSatisfy({
                MemoryFileTreeMenu.canRename($0, inOrgView: false)
                    && store.canEditMemory($0)
                    && !store.isSynchronizingDocument($0.id)
            }) else {
                throw MemoryDirectoryMutationError.readOnly
            }
            let resources = store.resources.filter {
                $0.scope == .org
                    || ($0.scope == .project && $0.projectId == store.activeProjectId)
            }
            let drafts = store.drafts.filter {
                $0.projectId == store.activeProjectId
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
            Task {
                var completed = 0
                directoryOperationProgress = "Renaming \(plan.changes.count) memories…"
                defer { directoryOperationProgress = nil }
                do {
                    for (index, change) in plan.changes.enumerated() {
                        directoryOperationProgress =
                            "Renaming \(index + 1) of \(plan.changes.count) memories…"
                        try await store.rename(change.item, to: change.newPath)
                        completed += 1
                    }
                } catch {
                    let prefix = completed == 0
                        ? ""
                        : "Renamed \(completed) of \(plan.changes.count) memories. "
                    store.errorMessage = prefix + error.localizedDescription
                }
            }
        } catch {
            store.errorMessage = error.localizedDescription
        }
    }

    private var selection: Binding<Set<String>> {
        Binding(
            get: { selectedNodeIds },
            set: { newSelection in
                let previous = selectedNodeIds
                selectedNodeIds = newSelection
                guard newSelection.count == 1,
                      let nodeId = newSelection.first,
                      let node = FileTreeNode.node(withId: nodeId, in: roots) else {
                    return
                }

                if newSelection != previous {
                    selectionAnchorId = nodeId
                }
                guard newSelection != previous, let item = node.item else { return }
                store.open(item)
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
            if expandedDirectoryIds.contains(nodeId) {
                expandedDirectoryIds.remove(nodeId)
            } else {
                expandedDirectoryIds.insert(nodeId)
            }
        }
    }

    @ViewBuilder
    private func fileTreeMenu(for nodeIds: Set<String>) -> some View {
        let targetItems = FileTreeNode.items(in: roots, selectedNodeIds: nodeIds)
        let exportItems = FileTreeNode.items(
            in: FileTreeNode.build(store.visibleMemoryItems),
            selectedNodeIds: nodeIds
        )
        let selectedDirectory = FileTreeNode.selectedDirectory(
            in: roots,
            selectedNodeIds: nodeIds
        )
        let singleItem = selectedDirectory == nil && targetItems.count == 1
            ? targetItems.first
            : nil
        let isOrgView = store.activeProjectId == nil
        let addableItems = MemoryFileTreeMenu.addable(targetItems, inOrgView: isOrgView)
        let removableItems = MemoryFileTreeMenu.removable(targetItems, inOrgView: isOrgView)
        let trashableItems = MemoryFileTreeMenu.trashable(targetItems, inOrgView: isOrgView)
            .filter { store.canEditMemory($0) }
        let singleRenameable = singleItem.map {
            MemoryFileTreeMenu.canRename($0, inOrgView: isOrgView)
                && store.canEditMemory($0)
        } ?? false
        let singleTrashable = singleItem.map { item in
            trashableItems.contains { $0.id == item.id }
        } ?? false
        let singleStale = singleItem.map { item in
            item.resource.map { store.staleResourceIds.contains($0.id) } == true
        } ?? false
        let singleSynchronizing = singleItem.map {
            store.isSynchronizingDocument($0.id)
        } ?? false
        let selectionContainsSynchronizingDocument = targetItems.contains {
            store.isSynchronizingDocument($0.id)
        }
        let trashSelectionContainsSynchronizingDocument = trashableItems.contains {
            store.isSynchronizingDocument($0.id)
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
                    && store.canEditMemory($0)
            }
        let directoryDeletionPlan = selectedDirectory.flatMap { _ in
            MemoryFileTreeMenu.directoryDeletionPlan(targetItems, inOrgView: isOrgView)
        }
        let directoryDeletionAllowed = directoryDeletionPlan?.itemsToDelete.allSatisfy {
            store.canEditMemory($0)
        } == true
        let hasDraftAction = !isOrgView
            && (singleItem?.draft != nil || !directoryDrafts.isEmpty)
        let hasDomainSection = !addableItems.isEmpty || !removableItems.isEmpty
            || hasDraftAction || !reviewDrafts.isEmpty || singleStale

        // ---- generic document operations (standard macOS conventions) ----
        if let selectedDirectory {
            if directoryRenameable {
                Button("Rename Folder…") { beginRenamingDirectory(selectedDirectory) }
                    .disabled(
                        directoryOperationProgress != nil
                            || selectionContainsSynchronizingDocument
                    )
            }
            if let directoryDeletionPlan, directoryDeletionAllowed {
                Button("Delete Folder…", role: .destructive) {
                    pendingAlert = .directoryDeletion(
                        name: selectedDirectory.name,
                        plan: directoryDeletionPlan
                    )
                }
                .disabled(
                    directoryOperationProgress != nil
                        || selectionContainsSynchronizingDocument
                )
            }
        } else if let singleItem {
            Button("Open") { store.open(singleItem) }
            if singleItem.supportsMarkdownPreview {
                Button("Open Source") { store.open(singleItem, mode: .source) }
            }
            if singleRenameable {
                Button("Rename…") { beginRenaming(singleItem) }
                    .disabled(directoryOperationProgress != nil || singleSynchronizing)
            }
            if singleTrashable {
                Button("Delete…", role: .destructive) {
                    proposeOrganizationDeletion([singleItem])
                }
                .disabled(directoryOperationProgress != nil || singleSynchronizing)
            }
        } else if !targetItems.isEmpty {
            Button("Open") { targetItems.forEach { store.open($0) } }
            if !trashableItems.isEmpty {
                Button(organizationDeletionTitle(count: trashableItems.count), role: .destructive) {
                    proposeOrganizationDeletion(trashableItems)
                }
                .disabled(
                    directoryOperationProgress != nil
                        || trashSelectionContainsSynchronizingDocument
                )
            }
        }

        if !exportItems.isEmpty {
            Button("Export as ZIP…") {
                store.exportMemory(
                    exportItems,
                    name: selectedDirectory?.name ?? singleItem?.document.title
                )
            }
            .disabled(directoryOperationProgress != nil || !store.canExportMemory(exportItems))
        }

        // ---- domain operations (Memory scope relationships and drafts) ----
        if hasDomainSection {
            Divider()
        }
        if !addableItems.isEmpty {
            Menu(addToProjectTitle(count: addableItems.count)) {
                if store.projects.isEmpty {
                    Button("No Projects") {}
                        .disabled(true)
                } else {
                    ForEach(store.projects) { project in
                        Button("Add to \(project.name)") {
                            addToProject(addableItems, projectId: project.id)
                        }
                        .disabled(!store.canManageProject(project.id))
                    }
                }
            }
            .disabled(
                directoryOperationProgress != nil
                    || !store.projects.contains(where: { store.canManageProject($0.id) })
                    || store.projects.isEmpty
                    || selectionContainsSynchronizingDocument
            )
        }
        if !removableItems.isEmpty {
            Button(removeFromProjectTitle(count: removableItems.count)) {
                removeFromProject(removableItems)
            }
            .disabled(
                directoryOperationProgress != nil
                    || store.activeProjectId.map { !store.canManageProject($0) } != false
                    || selectionContainsSynchronizingDocument
            )
            .help("Remove the reference from this project. The shared file is kept.")
        }
        if !isOrgView, !reviewDrafts.isEmpty {
            Button(reviewRequestTitle(count: reviewDrafts.count)) {
                pendingDirectoryReview = .init(
                    drafts: reviewDrafts,
                    initialTitle: directoryReviewTitle(for: nodeIds, draftCount: reviewDrafts.count)
                )
            }
            .disabled(
                directoryOperationProgress != nil
                    || !reviewSelectionIsReady
                    || selectionContainsSynchronizingDocument
            )
        }
        if !isOrgView, let draft = singleItem?.draft, draft.status == .submitted {
            Button("View Review") {
                Task { await store.openReview(for: draft) }
            }
        }
        if let selectedDirectory, !directoryDrafts.isEmpty {
            Button(
                directoryDrafts.count == 1
                    ? "Discard Draft in Folder…"
                    : "Discard \(directoryDrafts.count) Drafts in Folder…",
                role: .destructive
            ) {
                pendingAlert = .directoryDiscard(
                    name: selectedDirectory.name,
                    drafts: directoryDrafts
                )
            }
            .disabled(
                directoryOperationProgress != nil
                    || selectionContainsSynchronizingDocument
            )
        }
        if let singleItem {
            let resourceIsStale = singleItem.resource.map {
                store.staleResourceIds.contains($0.id)
            } == true
            if store.isSynchronizingDocument(singleItem.id) {
                Button("Preparing Shared Changes…") {}
                    .disabled(true)
            } else if let draft = singleItem.draft,
                      draft.freshness == .behind || resourceIsStale {
                switch draft.syncStatus {
                case .queued, .syncing, .retrying:
                    Button("Uploading Draft Changes…") {}
                        .disabled(true)
                case .failed:
                    if store.isRetryingSync(
                        channel: "drafts",
                        projectId: draft.projectId
                    ) {
                        Button("Retrying Draft Sync…") {}
                            .disabled(true)
                    } else {
                        Button("Retry Draft Sync") {
                            Task {
                                _ = await store.retrySync(
                                    channel: "drafts",
                                    projectId: draft.projectId
                                )
                            }
                        }
                        .disabled(directoryOperationProgress != nil)
                    }
                case .synced:
                    if draft.serverId == nil {
                        Button("Draft Not Ready") {}
                            .disabled(true)
                    } else {
                        Button(
                            draft.hasUpstreamResourceChanges
                                ? "Review Shared Changes"
                                : "Update from Shared Version"
                        ) {
                            guard directoryOperationProgress == nil else { return }
                            store.syncDocument(singleItem)
                        }
                        .disabled(directoryOperationProgress != nil)
                    }
                }
            } else if resourceIsStale {
                Button("Update from Shared Version") {
                    guard directoryOperationProgress == nil else { return }
                    store.syncDocument(singleItem)
                }
                .disabled(directoryOperationProgress != nil)
            }
            if !isOrgView, let draft = singleItem.draft {
                Button("Discard Draft") {
                    guard directoryOperationProgress == nil else { return }
                    Task { await store.discard(draft) }
                }
                .disabled(
                    directoryOperationProgress != nil
                        || singleSynchronizing
                )
            }
        }

        if targetItems.isEmpty {
            if let scope = MemoryFileTreeMenu.creationScope(inOrgView: isOrgView) {
                Button("Propose New Organization Memory") {
                    guard directoryOperationProgress == nil else { return }
                    Task {
                        await store.createMemory(kind: store.selectedKind, scope: scope)
                    }
                }
                .disabled(
                    directoryOperationProgress != nil
                        || !store.canCreateMemory(kind: store.selectedKind, scope: scope)
                )
            }
        }
    }

    private func synchronizeSelectionWithActiveItem() {
        guard selectedNodeIds.count <= 1,
              let itemId = store.activeVisibleTab?.itemId ?? store.selectedItemId,
              FileTreeNode.node(withId: itemId, in: roots) != nil else {
            return
        }
        selectedNodeIds = [itemId]
        selectionAnchorId = itemId
    }

    private func organizationDeletionTitle(count: Int) -> String {
        count == 1 ? "Delete…" : "Delete \(count) Files…"
    }

    private func reviewRequestTitle(count: Int) -> String {
        count == 1 ? "Request Review…" : "Request Review for \(count) Changes…"
    }

    private func directoryReviewTitle(for nodeIds: Set<String>, draftCount: Int) -> String {
        if nodeIds.count == 1,
           let nodeId = nodeIds.first,
           let node = FileTreeNode.node(withId: nodeId, in: roots),
           node.item == nil {
            return "Update \(node.name)"
        }
        return draftCount == 1 ? "Update memory" : "Update \(draftCount) memories"
    }

    private func proposeOrganizationDeletion(_ items: [MemoryListItem]) {
        guard !items.isEmpty else { return }
        pendingAlert = .organizationDeletion(items: items)
    }

    private func confirm(_ alert: MemoryFileTreeAlert) {
        dismissAlert()
        switch alert {
        case .itemRename, .directoryRename:
            return
        case .organizationDeletion(let items):
            deleteItems(items)
        case .directoryDiscard(_, let drafts):
            discardDrafts(drafts)
        case .directoryDeletion(_, let plan):
            deleteDirectory(plan)
        }
    }

    private func deleteItems(_ items: [MemoryListItem]) {
        guard directoryOperationProgress == nil else { return }
        Task {
            directoryOperationProgress = "Proposing \(items.count) deletions…"
            defer { directoryOperationProgress = nil }
            for (index, item) in items.enumerated() {
                directoryOperationProgress =
                    "Proposing deletion \(index + 1) of \(items.count)…"
                guard await store.delete(item) else { return }
            }
        }
    }

    private func discardDrafts(_ drafts: [LocalDraft]) {
        guard directoryOperationProgress == nil else { return }
        Task {
            directoryOperationProgress = "Discarding \(drafts.count) Drafts…"
            defer { directoryOperationProgress = nil }
            for (index, draft) in drafts.enumerated() {
                directoryOperationProgress =
                    "Discarding Draft \(index + 1) of \(drafts.count)…"
                guard await store.discard(draft) else {
                    let detail = store.errorMessage ?? "The remaining Drafts were not changed."
                    store.errorMessage = "Discarded \(index) of \(drafts.count) Drafts. " + detail
                    return
                }
            }
        }
    }

    private func deleteDirectory(_ plan: MemoryDirectoryDeletionPlan) {
        guard directoryOperationProgress == nil else { return }
        Task {
            let total = plan.itemsToDelete.count + plan.draftsToDiscard.count
            var completed = 0
            directoryOperationProgress = "Applying \(total) folder changes…"
            defer { directoryOperationProgress = nil }
            for item in plan.itemsToDelete {
                directoryOperationProgress =
                    "Applying folder change \(completed + 1) of \(total)…"
                guard await store.delete(item) else {
                    let detail = store.errorMessage ?? "The remaining files were not changed."
                    store.errorMessage =
                        "Completed \(completed) of \(total) folder changes. " + detail
                    return
                }
                completed += 1
            }
            for draft in plan.draftsToDiscard {
                directoryOperationProgress =
                    "Applying folder change \(completed + 1) of \(total)…"
                guard await store.discard(draft) else {
                    let detail = store.errorMessage ?? "The remaining files were not changed."
                    store.errorMessage =
                        "Completed \(completed) of \(total) folder changes. " + detail
                    return
                }
                completed += 1
            }
        }
    }

    private func addToProject(_ items: [MemoryListItem], projectId: String) {
        guard directoryOperationProgress == nil else { return }
        Task {
            do {
                try await store.addOrgMemories(
                    resourceIds: Set(items.map(\.id)),
                    toProject: projectId
                )
            } catch {
                store.errorMessage = error.localizedDescription
            }
        }
    }

    private func removeFromProject(_ items: [MemoryListItem]) {
        guard directoryOperationProgress == nil else { return }
        guard let projectId = store.activeProjectId else { return }
        Task {
            do {
                try await store.removeOrgMemories(
                    resourceIds: Set(items.map(\.id)),
                    fromProject: projectId
                )
            } catch {
                store.errorMessage = error.localizedDescription
            }
        }
    }

    private func addToProjectTitle(count: Int) -> String {
        count == 1 ? "Add to Project" : "Add \(count) Items to Project"
    }

    private func removeFromProjectTitle(count: Int) -> String {
        count == 1 ? "Remove from Project" : "Remove \(count) Items from Project"
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

enum MemoryFileTreeRowAccessory: Equatable {
    case none
    case legacyProjectReadOnly
    case draft
    case inReview

    static func resolve(item: MemoryListItem?) -> Self {
        if item?.resource?.scope == .project { return .legacyProjectReadOnly }
        switch item?.draft?.status {
        case .open: return .draft
        case .submitted: return .inReview
        default: return .none
        }
    }

    var help: String? {
        switch self {
        case .none: nil
        case .legacyProjectReadOnly: "Legacy Project memory — read-only"
        case .draft: "Draft — not submitted for review"
        case .inReview: "In Review — awaiting review and merge"
        }
    }
}

private struct FileTreeRow: View {
    let entry: VisibleFileTreeNode
    let isExpanded: Bool
    let isStale: Bool
    let review: ReviewRecord?
    let onOpenReview: () -> Void
    let onDirectoryClick: (NSEvent.ModifierFlags) -> Void

    private var item: MemoryListItem? { entry.node.item }

    @ViewBuilder
    var body: some View {
        if item == nil {
            rowContent
                .simultaneousGesture(
                    TapGesture().onEnded {
                        onDirectoryClick(NSEvent.modifierFlags)
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
            titleColor: titleColor
        ) {
            HStack(spacing: 5) {
                SharedUpdateIndicator(
                    freshness: item?.draft?.freshness,
                    hasUpstreamResourceChanges: item?.draft?.hasUpstreamResourceChanges == true,
                    reconciliation: item?.draft?.reconciliation,
                    isStale: isStale
                )
                if rowAccessory == .inReview {
                    Button(action: onOpenReview) {
                        DraftReviewIcon()
                            .frame(width: 20, height: 20)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help(review.map { "In Review: \($0.title). Click to view Review." }
                        ?? "In Review. Click to load and view Review.")
                    .accessibilityLabel("View Review for \(entry.node.name)")
                } else if rowAccessory == .draft {
                    DraftReviewIcon(submitted: false)
                        .help(rowAccessory.help ?? "Draft")
                        .accessibilityLabel("Draft — not submitted for review")
                } else if let help = rowAccessory.help {
                    Image(systemName: "lock.fill")
                        .font(.system(size: 9, weight: .medium))
                        .foregroundStyle(.secondary)
                        .help(help)
                        .accessibilityLabel(help)
                }
            }
        }
        .help(rowAccessory.help ?? entry.node.name)
    }

    private var titleColor: Color {
        MemoryFileTreeTitleTone.resolve(item: item).color
    }

    private var rowAccessory: MemoryFileTreeRowAccessory {
        MemoryFileTreeRowAccessory.resolve(item: item)
    }
}
