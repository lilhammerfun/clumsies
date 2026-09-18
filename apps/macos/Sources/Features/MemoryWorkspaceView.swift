import AppKit
import SwiftUI

struct MemoryNavigator: View {
    @ObservedObject var store: WorkspaceStore

    var body: some View {
        content
            .safeAreaInset(edge: .bottom) {
                DraftInventoryStatusBanner(store: store)
            }
            .onChange(of: store.selectedKind) { _, _ in
                store.selectedItemId = nil
            }
    }

    @ViewBuilder
    private var content: some View {
        if !query.isEmpty, items.isEmpty, store.isPreparingWorkspaceIndex {
            ProgressView("Preparing Search…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if !store.visibleMemoryItems.isEmpty, !query.isEmpty, items.isEmpty {
            ContentUnavailableView.search(text: query)
        } else {
            FileTreeView(store: store, items: items)
        }
    }

    private var query: String {
        store.searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var items: [MemoryListItem] {
        WorkspaceStore.filterMemoryItems(store.visibleMemoryItems, query: query)
    }
}

struct MemoryMainPane: View {
    @ObservedObject var store: WorkspaceStore

    var body: some View {
        VStack(spacing: 0) {
            if let project = store.activeProject, !project.isLoaded {
                ProjectPreparationView(store: store)
            } else if !store.visibleTabs.isEmpty {
                DocumentTabStrip(
                    tabs: store.visibleTabs,
                    selectedTabId: store.activeVisibleTab?.id,
                    onSelect: { tab in store.selectTab(tab) },
                    onClose: store.closeTab
                )
                .frame(
                    maxWidth: .infinity,
                    minHeight: DocumentTabMetrics.height,
                    maxHeight: DocumentTabMetrics.height,
                    alignment: .leading
                )
                .background(.bar)

                if let tab = store.activeVisibleTab,
                   let item = store.item(for: tab) {
                    let presentsUnavailableStaleDiff = tab.mode == .diff
                        && item.resource.map { store.staleResourceIds.contains($0.id) } == true
                    let presentsUnavailableDraftDiff = tab.mode == .diff
                        && item.draft?.documentBaselineAvailable == false
                    if item.contentLoaded || presentsUnavailableStaleDiff
                        || presentsUnavailableDraftDiff {
                        DocumentSessionView(store: store, item: item, mode: tab.mode)
                            .id(tab.id)
                    } else if item.draft?.documentBaselineAvailable == false {
                        ContentUnavailableView(
                            "Draft Source Unavailable",
                            systemImage: "arrow.trianglehead.2.clockwise.rotate.90",
                            description: Text(
                                "The shared file was removed. Open Diff or Sync to reconcile this draft safely."
                            )
                        )
                    } else {
                        ResourceLoadingView(store: store, item: item)
                            .id(item.id)
                    }
                } else {
                    emptyState
                }
            } else {
                emptyState
            }
        }
        .background(Color(nsColor: .textBackgroundColor))
    }

    @ViewBuilder
    private var emptyState: some View {
        if store.visibleMemoryItems.isEmpty {
            switch store.draftInventoryLoadState {
            case .loading:
                ContentLoadingView(title: "Loading Memory…")
            case .failed(let message):
                ContentUnavailableView {
                    Label("Drafts Unavailable", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(message)
                } actions: {
                    Button("Try Again") { Task { await store.reload() } }
                }
            case .loaded:
                EmptyMemoryCollectionView(store: store)
            }
        } else {
            EmptyWorkspaceView()
        }
    }
}

private struct DraftInventoryStatusBanner: View {
    @ObservedObject var store: WorkspaceStore

    @ViewBuilder
    var body: some View {
        switch store.draftInventoryLoadState {
        case .loading:
            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                Text("Loading Drafts...")
                    .font(.caption)
            }
            .padding(8)
            .frame(maxWidth: .infinity)
            .background(.bar)
        case .failed:
            Button("Draft refresh failed - Try Again") {
                Task { await store.reload() }
            }
            .buttonStyle(.plain)
            .font(.caption)
            .padding(8)
            .frame(maxWidth: .infinity)
            .background(.bar)
        case .loaded:
            EmptyView()
        }
    }
}

private struct ResourceLoadingView: View {
    @ObservedObject var store: WorkspaceStore
    let item: MemoryListItem
    @State private var failure: String?

    var body: some View {
        Group {
            if let failure {
                ContentUnavailableView {
                    Label("Memory Unavailable", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(failure)
                } actions: {
                    Button("Try Again") { Task { await load() } }
                }
            } else {
                ContentLoadingView(title: "Loading Memory…", layout: .document)
            }
        }
        .task(id: item) { await load() }
    }

    private func load() async {
        failure = nil
        let message = await store.loadContentIfNeeded(item)
        guard !Task.isCancelled else { return }
        failure = message
    }
}

private struct EmptyWorkspaceView: View {
    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "doc.text")
                .font(.system(size: 28, weight: .light))
                .foregroundStyle(.tertiary)
            Text("Open a memory from the navigator")
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct EmptyMemoryCollectionView: View {
    @ObservedObject var store: WorkspaceStore

    var body: some View {
        if store.activeProjectId != nil {
            MemoryGuidelinesSetupView(store: store)
        } else {
            ContentUnavailableView(
                "No Memory",
                systemImage: "doc",
                description: Text("Select a project to create memory and set up memory guidelines.")
            )
        }
    }
}

private struct ProjectPreparationView: View {
    @ObservedObject var store: WorkspaceStore

    var body: some View {
        if store.loadingProjectId == store.activeProjectId {
            ContentLoadingView(title: "Loading Project…")
        } else {
            ContentUnavailableView {
                Label("Project Unavailable", systemImage: "folder.badge.questionmark")
            } description: {
                Text("The project could not be loaded. Try again when the connection is available.")
            } actions: {
                if let projectId = store.activeProjectId {
                    Button("Try Again") {
                        Task { await store.selectProject(projectId) }
                    }
                }
            }
        }
    }
}

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

private struct FileTreeView: View {
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

struct DraftReviewIcon: View {
    var submitted = true

    private static let openImage = load("git-pull-request-16")
    private static let draftImage = load("git-pull-request-draft-16")

    private static func load(_ name: String) -> NSImage? {
        Bundle.main.url(forResource: name, withExtension: "svg", subdirectory: "Octicons")
            .flatMap { NSImage(contentsOf: $0) }
    }

    var body: some View {
        Group {
            if let image = submitted ? Self.openImage : Self.draftImage {
                Image(nsImage: image)
                    .renderingMode(.template)
                    .resizable()
                    .scaledToFit()
            } else {
                Image(systemName: "checkmark.bubble")
                    .resizable()
                    .scaledToFit()
            }
        }
        .frame(width: 14, height: 14)
        .foregroundStyle(submitted ? Color(nsColor: .systemGreen) : .secondary)
    }
}

struct FileTreeDirectoryClickResult {
    let selection: Set<String>
    let anchorId: String?
    let togglesDirectory: Bool
}

enum FileTreeSelectionInteraction {
    static func directoryClick(
        nodeId: String,
        visibleNodeIds: [String],
        currentSelection: Set<String>,
        anchorId: String?,
        modifierFlags: NSEvent.ModifierFlags
    ) -> FileTreeDirectoryClickResult {
        if modifierFlags.contains(.shift) {
            let effectiveAnchor = anchorId ?? nodeId
            guard let anchorIndex = visibleNodeIds.firstIndex(of: effectiveAnchor),
                  let nodeIndex = visibleNodeIds.firstIndex(of: nodeId) else {
                return .init(
                    selection: [nodeId],
                    anchorId: nodeId,
                    togglesDirectory: false
                )
            }
            let range = min(anchorIndex, nodeIndex) ... max(anchorIndex, nodeIndex)
            let rangeSelection = Set(range.map { visibleNodeIds[$0] })
            return .init(
                selection: modifierFlags.contains(.command)
                    ? currentSelection.union(rangeSelection)
                    : rangeSelection,
                anchorId: effectiveAnchor,
                togglesDirectory: false
            )
        }

        if modifierFlags.contains(.command) {
            var selection = currentSelection
            if selection.contains(nodeId) {
                selection.remove(nodeId)
            } else {
                selection.insert(nodeId)
            }
            return .init(
                selection: selection,
                anchorId: nodeId,
                togglesDirectory: false
            )
        }

        guard modifierFlags.intersection([.option, .control]).isEmpty else {
            return .init(
                selection: currentSelection,
                anchorId: anchorId,
                togglesDirectory: false
            )
        }

        return .init(
            selection: [nodeId],
            anchorId: nodeId,
            togglesDirectory: true
        )
    }
}

struct VisibleFileTreeNode: Identifiable {
    let node: FileTreeNode
    let depth: Int

    var id: String { node.id }
}

struct FileTreeNode: Identifiable {
    let id: String
    let name: String
    let item: MemoryListItem?
    let children: [FileTreeNode]?

    static func build(_ items: [MemoryListItem]) -> [FileTreeNode] {
        var itemsById: [String: MemoryListItem] = [:]
        let pathItems = items.map { item in
            itemsById[item.id] = item
            return PathTreeItem(
                id: item.id,
                path: treePath(for: item),
                fallbackName: item.document.title
            )
        }

        func convert(_ node: PathTreeNode) -> FileTreeNode {
            FileTreeNode(
                id: node.id,
                name: node.name,
                item: node.item.flatMap { itemsById[$0.id] },
                children: node.children.map { $0.map(convert) }
            )
        }

        return PathTreeNode.build(pathItems).map(convert)
    }

    static func treePath(for item: MemoryListItem) -> String {
        treePath(for: item.document.path, kind: item.kind)
    }

    static func treePath(for documentPath: String, kind: MemoryKind) -> String {
        var components = documentPath.split(separator: "/").map(String.init)
        if kind == .workflows, components.first == "workflow", components.count > 1 {
            components.removeFirst()
        }
        return components.joined(separator: "/")
    }

    static func documentPath(fromTreePath path: String, for item: MemoryListItem) -> String {
        guard item.kind == .workflows,
              item.document.path.split(separator: "/").first == "workflow" else {
            return path
        }
        return "workflow/\(path)"
    }

    static func directoryPath(from id: String) -> String? {
        let prefix = "directory:"
        guard id.hasPrefix(prefix) else { return nil }
        let path = String(id.dropFirst(prefix.count))
        return path.isEmpty ? nil : path
    }

    static func directoryIds(in nodes: [FileTreeNode]) -> Set<String> {
        nodes.reduce(into: Set<String>()) { result, node in
            guard let children = node.children else { return }
            result.insert(node.id)
            result.formUnion(directoryIds(in: children))
        }
    }

    static func allIds(in nodes: [FileTreeNode]) -> [String] {
        nodes.flatMap { node in
            [node.id] + (node.children.map { allIds(in: $0) } ?? [])
        }
    }

    static func node(withId id: String, in nodes: [FileTreeNode]) -> FileTreeNode? {
        for node in nodes {
            if node.id == id { return node }
            if let children = node.children,
               let match = self.node(withId: id, in: children) {
                return match
            }
        }
        return nil
    }

    static func selectedDirectory(
        in nodes: [FileTreeNode],
        selectedNodeIds: Set<String>
    ) -> FileTreeNode? {
        guard selectedNodeIds.count == 1,
              let id = selectedNodeIds.first,
              let node = node(withId: id, in: nodes),
              node.item == nil else {
            return nil
        }
        return node
    }

    static func items(
        in nodes: [FileTreeNode],
        selectedNodeIds: Set<String>
    ) -> [MemoryListItem] {
        var selectedItems: [String: MemoryListItem] = [:]

        func collect(_ node: FileTreeNode) {
            if let item = node.item {
                selectedItems[item.id] = item
            }
            node.children?.forEach(collect)
        }

        func visit(_ node: FileTreeNode) {
            if selectedNodeIds.contains(node.id) {
                collect(node)
            } else {
                node.children?.forEach(visit)
            }
        }

        nodes.forEach(visit)
        return selectedItems.values.sorted {
            $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending
        }
    }

    static func visibleNodes(
        _ nodes: [FileTreeNode],
        expandedDirectoryIds: Set<String>,
        depth: Int = 0
    ) -> [VisibleFileTreeNode] {
        nodes.flatMap { node in
            var result = [VisibleFileTreeNode(node: node, depth: depth)]
            if expandedDirectoryIds.contains(node.id), let children = node.children {
                result.append(contentsOf: visibleNodes(
                    children,
                    expandedDirectoryIds: expandedDirectoryIds,
                    depth: depth + 1
                ))
            }
            return result
        }
    }
}

private struct DocumentDiffIdentity: Hashable {
    let item: MemoryListItem
    let localDocument: EditableMemoryDocument
    let staleResourceGeneration: UUID?
    let retryRequest: Int
}

private struct DocumentSessionView: View {
    @ObservedObject var store: WorkspaceStore
    let item: MemoryListItem
    let mode: WorkbenchTabMode

    @State private var document: EditableMemoryDocument
    @State private var authoritativeDocument: EditableMemoryDocument
    @State private var suppressesSaving = false
    @State private var reviewDraft: LocalDraft?
    @State private var reconciliationUpdateRequest = 0
    @State private var documentDiffPresentation: UnifiedDiffPresentation?
    @State private var documentPathChanges: [DocumentPathChange] = []
    @State private var loadsDocumentDiff = false
    @State private var documentDiffError: String?
    @State private var documentDiffRetryRequest = 0
    @State private var confirmsOrganizationDeletion = false

    private var sessionKey: MemoryDocumentSessionKey? {
        store.documentSessionKey(for: item)
    }

    init(store: WorkspaceStore, item: MemoryListItem, mode: WorkbenchTabMode) {
        self.store = store
        self.item = item
        self.mode = mode
        _document = State(initialValue: store.pendingDocument(for: item) ?? item.document)
        _authoritativeDocument = State(initialValue: item.document)
    }

    var body: some View {
        Group {
            if let candidate = store.pendingDocumentReconciliationCandidates[item.id] {
                DraftReconciliationView(
                    candidate: candidate,
                    updateRequest: reconciliationUpdateRequest,
                    usesContextualUpdateAction: true,
                    initialResolvedState: store.documentReconciliationResolution(for: item.id),
                    onResolvedStateChange: {
                        store.updateDocumentReconciliationResolution($0, for: item.id)
                    },
                    onUpdateStateChange: publishReconciliationToolbarState,
                    onCancel: closeReconciliation,
                    onApplied: closeReconciliation
                ) { resolvedState in
                    try await store.applyReconciliation(
                        draftId: candidate.draftId,
                        candidate: candidate,
                        resolvedState: resolvedState,
                        documentItemId: item.id
                    )
                }
                .id(candidate.candidateId)
            } else {
                documentContent
            }
        }
        .onChange(of: item.document) { _, latest in
            adoptAuthoritativeDocument(latest)
        }
        .onChange(of: store.documentContentGeneration(for: item.id)) { _, _ in
            adoptAuthoritativeDocument(item.document)
        }
        .onChange(of: store.pendingDocumentCommand) { _, command in
            handleDocumentCommand(command)
        }
        .onAppear {
            handleDocumentCommand(store.pendingDocumentCommand)
        }
        .onDisappear {
            flushSave()
            clearReconciliationToolbarState()
        }
        .sheet(item: $reviewDraft) { draft in
            ReviewRequestSheet(
                initialTitle: document.title,
                loadCandidates: { [try await loadReviewCandidate(draft)] }
            ) { title, description, reconciliations in
                let reconciliation = reconciliations.first
                try await submitReview(
                    draft,
                    title: title,
                    description: description,
                    candidate: reconciliation?.candidate,
                    resolvedState: reconciliation?.resolvedState
                )
            }
        }
        .alert("Delete File?", isPresented: $confirmsOrganizationDeletion) {
            Button("Cancel", role: .cancel) {}
            Button("Delete", role: .destructive) {
                moveToTrash()
            }
        } message: {
            Text(
                "This creates a deletion draft proposal. If reviewed and merged, "
                    + "the organization memory will be removed for every project that includes it."
            )
        }
    }

    private var documentContent: some View {
        Group {
            if mode == .diff {
                documentDiff
            } else if item.draft?.isDeletion == true {
                ContentUnavailableView(
                    item.draft?.scope == .org
                        ? "Pending organization deletion"
                        : "Pending deletion",
                    systemImage: "trash",
                    description: Text(
                        item.draft?.scope == .org
                            ? "Discard the draft proposal to keep this organization memory."
                            : "Discard the draft to keep this memory."
                    )
                )
            } else if mode == .preview {
                MarkdownPreview(source: renderedSource)
            } else {
                editor
                    .disabled(
                        !store.canEditMemory(item)
                            || store.isSwitchingMemoryContext
                            || store.isSynchronizingDocument(item.id)
                    )
            }
        }
    }

    @ViewBuilder
    private var documentDiff: some View {
        let identity = documentDiffIdentity
        // The pane contract for a document view is the same one Preview
        // uses: the root is a greedy vertical ScrollView that fills the
        // remaining height, with content pinned to the top. The unified
        // diff stays a content-sized fragment (as in Reviews) and this
        // outer scroll handles vertical overflow.
        GeometryReader { geometry in
            ScrollView([.vertical]) {
                VStack(alignment: .leading, spacing: 0) {
                    if !documentPathChanges.isEmpty {
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(documentPathChanges.indices, id: \.self) { index in
                                HStack(spacing: 6) {
                                    Image(systemName: "arrow.right")
                                        .foregroundStyle(.secondary)
                                    Text(pathChangeSummary(documentPathChanges[index]))
                                        .textSelection(.enabled)
                                }
                            }
                        }
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 12)
                        .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
                        .background(Color.accentColor.opacity(0.06))
                    }

                    if let presentation = documentDiffPresentation,
                       presentation.changedLineCount > 0 {
                        UnifiedDiffView(presentation: presentation)
                    } else if loadsDocumentDiff {
                        ProgressView()
                            .controlSize(.small)
                    } else if let documentDiffError {
                        ContentUnavailableView {
                            Label("Unable to Load Diff", systemImage: "exclamationmark.triangle")
                        } description: {
                            Text(documentDiffError)
                        } actions: {
                            Button("Retry") { documentDiffRetryRequest += 1 }
                        }
                    } else if documentPathChanges.isEmpty {
                        ContentUnavailableView(
                            "No Changes",
                            systemImage: "doc.text",
                            description: Text("No local or remote changes to show for this document.")
                        )
                    }
                }
                .frame(
                    maxWidth: .infinity,
                    minHeight: geometry.size.height,
                    alignment: centersDocumentDiffStatus ? .center : .topLeading
                )
            }
        }
        .task(id: identity) {
            await loadDocumentDiff(for: identity)
        }
    }

    private var centersDocumentDiffStatus: Bool {
        documentPathChanges.isEmpty
            && (documentDiffPresentation?.changedLineCount ?? 0) == 0
    }

    /// A path-only change has no content diff. Surface draft and shared
    /// additions, removals, and renames so the pane never looks empty.
    private func pathChangeSummary(_ change: DocumentPathChange) -> String {
        let owner = switch change.source {
        case .draft: "Draft"
        case .shared: "Shared"
        case .draftAndShared: "Draft + Shared"
        }
        switch (change.from, change.to) {
        case let (from?, to?):
            return "\(owner) path: \(from) → \(to)"
        case let (nil, to?):
            return "\(owner) added: \(to)"
        case let (from?, nil):
            return "\(owner) deleted: \(from)"
        case (nil, nil):
            return owner
        }
    }

    private var documentDiffIdentity: DocumentDiffIdentity {
        DocumentDiffIdentity(
            item: item,
            localDocument: document,
            staleResourceGeneration: item.resource.flatMap {
                store.staleResourceGeneration(for: $0.id)
            },
            retryRequest: documentDiffRetryRequest
        )
    }

    private func loadDocumentDiff(for identity: DocumentDiffIdentity) async {
        guard mode == .diff else { return }
        documentDiffPresentation = nil
        documentDiffError = nil
        documentPathChanges = store.documentPathChanges(for: identity.item)
        loadsDocumentDiff = true

        do {
            let result = try await store.documentDiffPresentation(
                for: identity.item,
                localText: identity.localDocument.body
            )
            try Task.checkCancellation()
            guard identity == documentDiffIdentity, mode == .diff else { return }
            documentDiffPresentation = result?.presentation
            documentPathChanges = result?.pathChanges ?? []
            loadsDocumentDiff = false
        } catch is CancellationError {
            // `.task(id:)` immediately starts a replacement for a changed
            // identity. Let that task remain the owner of loading state.
        } catch {
            guard !Task.isCancelled,
                  identity == documentDiffIdentity,
                  mode == .diff else { return }
            documentDiffError = error.localizedDescription
            loadsDocumentDiff = false
        }
    }

    @ViewBuilder
    private var editor: some View {
        switch item.kind {
        case .context, .rules, .workflows:
            NativeTextEditor(text: editorText)
        }
    }

    private var editorText: Binding<String> {
        Binding(
            get: { document.body },
            set: { nextBody in
                guard nextBody != document.body else { return }
                var nextDocument = document
                nextDocument.body = nextBody
                document = nextDocument
                stageSave(nextDocument)
            }
        )
    }

    private var renderedSource: String {
        switch item.kind {
        case .context, .rules, .workflows:
            return document.body
        }
    }

    private func adoptAuthoritativeDocument(_ latest: EditableMemoryDocument) {
        let previous = authoritativeDocument
        authoritativeDocument = latest
        // A resource sync can replace the authoritative document while this
        // session stays alive. Adopt it only when the editor still matches the
        // previous snapshot so an in-flight local edit is never lost.
        if document == previous {
            document = latest
            return
        }
        // A file-tree rename is independent of a dirty Source body. Merge
        // path/title changes from the authoritative draft while keeping the
        // user's in-flight text, so the next autosave cannot rename it back.
        if document.path == previous.path {
            document.path = latest.path
        }
        if document.title == previous.title {
            document.title = latest.title
        }
    }

    private func handleDocumentCommand(_ command: DocumentSessionCommand?) {
        guard let command, command.sessionKey == sessionKey else { return }
        store.pendingDocumentCommand = nil
        switch command {
        case .requestReview(_, let draft):
            reviewDraft = draft
        case .discardDraft(_, let draft):
            discard(draft)
        case .applyReconciliation:
            reconciliationUpdateRequest += 1
        case .closeReconciliation:
            closeReconciliation()
        case .moveToTrash:
            guard store.canEditMemory(item),
                  MemoryFileTreeMenu.canProposeOrganizationDeletion(
                      item,
                      inOrgView: store.activeProjectId == nil
                  ) else {
                return
            }
            confirmsOrganizationDeletion = true
        }
    }

    private func publishReconciliationToolbarState(canUpdate: Bool, isUpdating: Bool) {
        guard let sessionKey else { return }
        store.documentReconciliationToolbarState = .init(
            sessionKey: sessionKey,
            isLoading: false,
            canUpdate: canUpdate,
            isUpdating: isUpdating
        )
    }

    private func closeReconciliation() {
        guard let sessionKey else { return }
        store.finishDocumentReconciliation(for: sessionKey)
        clearReconciliationToolbarState()
    }

    private func clearReconciliationToolbarState() {
        guard store.documentReconciliationToolbarState?.sessionKey == sessionKey else { return }
        store.documentReconciliationToolbarState = nil
    }

    private func stageSave(_ nextDocument: EditableMemoryDocument) {
        guard !suppressesSaving,
              store.canEditMemory(item),
              !store.isSwitchingMemoryContext,
              !store.isSynchronizingDocument(item.id),
              mode == .source,
              nextDocument != item.document else { return }
        store.stageDocumentSave(item, document: nextDocument)
    }

    private func flushSave() {
        guard !suppressesSaving,
              mode == .source else { return }
        Task {
            do {
                try await store.flushDocumentSave(item)
            } catch {
                store.errorMessage = error.localizedDescription
            }
        }
    }

    private func submitReview(
        _ draft: LocalDraft,
        title: String,
        description: String,
        candidate: DraftReconciliationCandidate?,
        resolvedState: ReconciliationResourceState?
    ) async throws {
        try await store.flushDocumentSave(item)
        let latest = store.drafts.first { $0.id == draft.id } ?? draft
        try await store.requestReview(
            for: latest,
            title: title,
            description: description,
            candidate: candidate,
            resolvedState: resolvedState
        )
    }

    private func loadReviewCandidate(_ draft: LocalDraft) async throws -> DraftReconciliationCandidate {
        try await store.flushDocumentSave(item)
        let latest = store.drafts.first { $0.id == draft.id } ?? draft
        return try await store.reconciliationCandidate(for: latest)
    }

    private func discard(_ draft: LocalDraft) {
        suppressesSaving = true
        store.cancelDocumentSave(item)
        Task {
            await store.discard(draft)
            suppressesSaving = false
        }
    }

    private func moveToTrash() {
        guard let activeProjectId = store.activeProjectId,
              item.projectContextId == activeProjectId,
              store.canEditMemory(item),
              MemoryFileTreeMenu.canProposeOrganizationDeletion(
                  item,
                  inOrgView: false
              ) else {
            return
        }
        suppressesSaving = true
        store.cancelDocumentSave(item)
        Task {
            await store.delete(item)
            suppressesSaving = false
        }
    }
}

struct DraftReconciliationView: View {
    let candidate: DraftReconciliationCandidate
    let updateRequest: Int
    let usesContextualUpdateAction: Bool
    let onResolvedStateChange: ((ReconciliationResourceState) -> Void)?
    let onUpdateStateChange: ((Bool, Bool) -> Void)?
    let onCancel: () -> Void
    let onApplied: () -> Void
    let onApply: (ReconciliationResourceState?) async throws -> Void

    @State private var resolvedExists: Bool
    @State private var resolvedPath: String
    @State private var resolvedContent: String
    @State private var isApplying = false
    @State private var errorMessage: String?

    init(
        candidate: DraftReconciliationCandidate,
        updateRequest: Int = 0,
        usesContextualUpdateAction: Bool = false,
        initialResolvedState: ReconciliationResourceState? = nil,
        onResolvedStateChange: ((ReconciliationResourceState) -> Void)? = nil,
        onUpdateStateChange: ((Bool, Bool) -> Void)? = nil,
        onCancel: @escaping () -> Void,
        onApplied: (() -> Void)? = nil,
        onApply: @escaping (ReconciliationResourceState?) async throws -> Void
    ) {
        self.candidate = candidate
        self.updateRequest = updateRequest
        self.usesContextualUpdateAction = usesContextualUpdateAction
        self.onResolvedStateChange = onResolvedStateChange
        self.onUpdateStateChange = onUpdateStateChange
        self.onCancel = onCancel
        self.onApplied = onApplied ?? onCancel
        self.onApply = onApply
        let initial = initialResolvedState ?? candidate.proposedState ?? candidate.draftState
        _resolvedExists = State(initialValue: initial.exists)
        _resolvedPath = State(initialValue: initial.resource.path ?? "")
        _resolvedContent = State(
            initialValue: Self.resolutionContentTemplate(
                for: candidate,
                preferredState: initial
            ).primaryText
        )
    }

    var body: some View {
        VStack(spacing: 0) {
            if !candidate.valid {
                HStack(spacing: 7) {
                    Image(systemName: "arrow.trianglehead.2.clockwise.rotate.90")
                    Text("A newer shared version is available. Review the latest update again.")
                    Spacer()
                }
                .font(.caption)
                .foregroundStyle(.orange)
                .padding(.horizontal, 10)
                .frame(height: 34)
                Divider()
            }

            if candidate.status == .conflicts {
                conflictResolution
            } else {
                cleanDiff
            }

            if !usesContextualUpdateAction {
                Divider()
                HStack {
                    Button("Cancel") { onCancel() }
                        .keyboardShortcut(.cancelAction)
                    Spacer()
                    Button {
                        apply()
                    } label: {
                        if isApplying {
                            ProgressView().controlSize(.small)
                        } else {
                            Text("Update")
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canApply)
                }
                .padding(12)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { publishUpdateState() }
        .onChange(of: canApply) { _, _ in publishUpdateState() }
        .onChange(of: isApplying) { _, _ in publishUpdateState() }
        .onChange(of: resolvedExists) { _, _ in publishResolution() }
        .onChange(of: resolvedPath) { _, _ in publishResolution() }
        .onChange(of: resolvedContent) { _, _ in publishResolution() }
        .onChange(of: updateRequest) { _, _ in
            guard usesContextualUpdateAction else { return }
            apply()
        }
        .alert(
            "Could Not Update Draft",
            isPresented: Binding(
                get: { errorMessage != nil },
                set: { if !$0 { errorMessage = nil } }
            )
        ) {
            Button("OK") { errorMessage = nil }
        } message: {
            Text(errorMessage ?? "")
                .textSelection(.enabled)
        }
    }

    private var canApply: Bool {
        !isApplying
            && candidate.valid
            && !(candidate.status == .conflicts && resolvedExists && resolvedPath.isEmpty)
    }

    private func publishUpdateState() {
        guard usesContextualUpdateAction else { return }
        onUpdateStateChange?(canApply, isApplying)
    }

    private func publishResolution() {
        onResolvedStateChange?(resolvedState)
    }

    @ViewBuilder
    private var cleanDiff: some View {
        let states = candidate.postSyncDiffStates
        if states.base != states.draft {
            reconciliationDiff(
                from: states.base,
                to: states.draft,
                title: "Shared Version → Updated Draft"
            )
        } else {
            ContentUnavailableView(
                "No Draft Changes",
                systemImage: "doc.text",
                description: Text(
                    "Updating moves this draft to the latest shared version without leaving changes to this file."
                )
            )
        }
    }

    private var conflictResolution: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Label(conflictSummary, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)

                Spacer(minLength: 12)

                if hasExistenceConflict {
                    Toggle("Keep File", isOn: $resolvedExists)
                        .toggleStyle(.switch)
                        .controlSize(.small)
                }

                if resolvedExists && hasPathConflict {
                    TextField("Path", text: $resolvedPath)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 280)
                }
            }
            .padding(.horizontal, 10)
            .frame(height: 34)
            Divider()

            VSplitView {
                reconciliationDiff(
                    from: candidate.currentState,
                    to: resolvedState,
                    title: "Shared Version → Resolution Preview"
                )
                .frame(minHeight: 220, maxHeight: .infinity)

                resolvedContentPane
                    .frame(minHeight: 180, maxHeight: .infinity)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func reconciliationDiff(
        from originalState: ReconciliationResourceState,
        to modifiedState: ReconciliationResourceState,
        title: String
    ) -> some View {
        let originalPath = path(in: originalState)
        let modifiedPath = path(in: modifiedState)
        return GeometryReader { geometry in
            ScrollView(.vertical) {
                VStack(alignment: .leading, spacing: 0) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(title)
                            .font(.caption.weight(.medium))
                        if originalPath != modifiedPath {
                            Text("\(originalPath ?? "/dev/null") → \(modifiedPath ?? "/dev/null")")
                                .font(.caption2.monospaced())
                                .foregroundStyle(.secondary)
                                .textSelection(.enabled)
                        }
                    }
                    .padding(.horizontal, 12)
                    .frame(
                        maxWidth: .infinity,
                        minHeight: originalPath == modifiedPath ? 28 : 42,
                        alignment: .leading
                    )
                    .background(Color.accentColor.opacity(0.06))

                    UnifiedDiffView(
                        presentation: UnifiedDiffPresentation(
                            model: SplitDiffModel.make(
                                original: text(in: originalState),
                                modified: text(in: modifiedState)
                            )
                        )
                    )
                }
                .frame(
                    maxWidth: .infinity,
                    minHeight: geometry.size.height,
                    alignment: .topLeading
                )
            }
        }
    }

    private var resolvedContentPane: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Resolve Draft")
                .font(.caption.weight(.medium))
                .padding(.horizontal, 10)
                .frame(maxWidth: .infinity, minHeight: 32, alignment: .leading)
            Divider()

            if resolvedExists {
                TextEditor(text: $resolvedContent)
                    .font(.system(.body, design: .monospaced))
                    .scrollContentBackground(.hidden)
                    .background(Color(nsColor: .textBackgroundColor))
            } else {
                ContentUnavailableView(
                    "File Removed",
                    systemImage: "trash",
                    description: Text("The resolved result removes this file.")
                )
            }
        }
    }

    private var conflictSummary: String {
        let fields = Array(Set(candidate.conflicts.map(\.field))).sorted()
        let noun = candidate.conflicts.count == 1 ? "conflict" : "conflicts"
        guard !fields.isEmpty else { return "\(candidate.conflicts.count) \(noun)" }
        return "\(candidate.conflicts.count) \(noun): \(fields.joined(separator: ", "))"
    }

    private var hasExistenceConflict: Bool {
        candidate.conflicts.contains { $0.field == "exists" }
    }

    private var hasPathConflict: Bool {
        candidate.conflicts.contains { $0.field == "path" || $0.field == "path_occupied" }
    }

    private func text(in state: ReconciliationResourceState) -> String {
        state.exists ? state.content?.primaryText ?? "" : ""
    }

    private func path(in state: ReconciliationResourceState) -> String? {
        state.exists ? state.resource.path : nil
    }

    private func apply() {
        guard canApply else { return }
        isApplying = true
        Task {
            defer { isApplying = false }
            do {
                let resolved = candidate.status == .conflicts ? resolvedState : nil
                try await onApply(resolved)
                onApplied()
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    private var resolvedState: ReconciliationResourceState {
        let template = candidate.proposedState ?? candidate.draftState
        let contentTemplate = Self.resolutionContentTemplate(
            for: candidate,
            preferredState: template
        )
        let resource = ServerDraftResourceReference(
            scope: template.resource.scope,
            id: template.resource.id,
            path: resolvedExists ? resolvedPath : template.resource.path
        )
        return .init(
            exists: resolvedExists,
            resource: resource,
            content: resolvedExists
                ? contentTemplate.replacingPrimaryText(with: resolvedContent)
                : nil
        )
    }

    static func resolutionContentTemplate(
        for candidate: DraftReconciliationCandidate,
        preferredState: ReconciliationResourceState
    ) -> DaemonDraftContent {
        preferredState.content
            ?? candidate.currentState.content
            ?? candidate.draftState.content
            ?? candidate.baseState.content
            ?? .init(description: nil, content: "")
    }
}

struct ReviewRequestSheet: View {
    @Environment(\.dismiss) private var dismiss

    let loadCandidates: () async throws -> [DraftReconciliationCandidate]
    let onSubmit: (String, String, [ReviewDraftReconciliation]) async throws -> Void

    @State private var title: String
    @State private var description = ""
    @State private var isSubmitting = false
    @State private var errorMessage: String?
    @State private var reconciliationCandidates = [DraftReconciliationCandidate]()
    @State private var resolvedStatesByCandidateId = [String: ReconciliationResourceState]()
    @State private var conflictIndex = 0

    init(
        initialTitle: String,
        loadCandidates: @escaping () async throws -> [DraftReconciliationCandidate],
        onSubmit: @escaping (String, String, [ReviewDraftReconciliation]) async throws -> Void
    ) {
        _title = State(initialValue: initialTitle)
        self.loadCandidates = loadCandidates
        self.onSubmit = onSubmit
    }

    var body: some View {
        Group {
            if reconciliationCandidates.count == 1,
               let candidate = reconciliationCandidates.first {
                DraftReconciliationView(
                    candidate: candidate,
                    onCancel: resetReconciliation,
                    onApplied: { dismiss() }
                ) { resolvedState in
                    try await onSubmit(
                        normalizedTitle,
                        normalizedDescription,
                        [.init(candidate: candidate, resolvedState: resolvedState)]
                    )
                }
                .frame(minWidth: 780, idealWidth: 980, minHeight: 560, idealHeight: 680)
            } else if let candidate = activeConflictCandidate {
                DraftReconciliationView(
                    candidate: candidate,
                    onCancel: resetReconciliation,
                    onApplied: { conflictIndex += 1 }
                ) { resolvedState in
                    if let resolvedState {
                        resolvedStatesByCandidateId[candidate.candidateId] = resolvedState
                    }
                }
                .id(candidate.candidateId)
                .frame(minWidth: 780, idealWidth: 980, minHeight: 560, idealHeight: 680)
            } else if !reconciliationCandidates.isEmpty {
                batchConfirmation
            } else {
                requestForm
            }
        }
        .interactiveDismissDisabled(isSubmitting)
        .alert(
            "Could Not Request Review",
            isPresented: Binding(
                get: { errorMessage != nil },
                set: { if !$0 { errorMessage = nil } }
            )
        ) {
            Button("OK") { errorMessage = nil }
        } message: {
            Text(errorMessage ?? "")
                .textSelection(.enabled)
        }
    }

    private var requestForm: some View {
        VStack(spacing: 0) {
            Form {
                Section("Review") {
                    TextField("Title", text: $title)
                    TextField("Description", text: $description, axis: .vertical)
                        .lineLimit(4...8)
                }
            }
            .formStyle(.grouped)

            Divider()

            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button {
                    submit()
                } label: {
                    if isSubmitting {
                        ProgressView()
                            .controlSize(.small)
                    } else {
                        Text("Request")
                    }
                }
                .disabled(isSubmitting || normalizedTitle.isEmpty)
                .keyboardShortcut(.defaultAction)
            }
            .padding(12)
        }
        .frame(width: 480, height: 270)
    }

    private var batchConfirmation: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Update Drafts and Request Review")
                    .font(.title2.weight(.semibold))
                Text("The latest shared changes will be applied to these drafts in one transaction.")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(20)

            Divider()

            List(reconciliationCandidates) { candidate in
                HStack(spacing: 10) {
                    Image(systemName: candidate.status == .conflicts
                        ? "checkmark.circle.fill"
                        : "arrow.trianglehead.merge")
                        .foregroundStyle(candidate.status == .conflicts ? .green : .secondary)
                    Text(candidatePath(candidate))
                        .font(.body.monospaced())
                    Spacer()
                    Text(candidate.status == .conflicts ? "Resolved" : "Clean")
                        .foregroundStyle(.secondary)
                }
            }

            Divider()

            HStack {
                Button("Back") { resetReconciliation() }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button {
                    submitBatch()
                } label: {
                    if isSubmitting {
                        ProgressView().controlSize(.small)
                    } else {
                        Text("Update and Request Review")
                    }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(isSubmitting || reconciliationCandidates.contains { !$0.valid })
            }
            .padding(12)
        }
        .frame(width: 620, height: 460)
    }

    private var normalizedTitle: String {
        title.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var normalizedDescription: String {
        description.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var activeConflictCandidate: DraftReconciliationCandidate? {
        let conflicts = reconciliationCandidates.filter { $0.status == .conflicts }
        guard conflictIndex < conflicts.count else { return nil }
        return conflicts[conflictIndex]
    }

    private func candidatePath(_ candidate: DraftReconciliationCandidate) -> String {
        candidate.proposedState?.resource.path
            ?? candidate.draftState.resource.path
            ?? candidate.currentState.resource.path
            ?? candidate.draftId
    }

    private func resetReconciliation() {
        reconciliationCandidates = []
        resolvedStatesByCandidateId = [:]
        conflictIndex = 0
    }

    private func submit() {
        guard !normalizedTitle.isEmpty else { return }
        isSubmitting = true
        Task {
            do {
                try await onSubmit(normalizedTitle, normalizedDescription, [])
                dismiss()
            } catch ReviewRequestError.reconciliationRequired {
                do {
                    let candidates = try await loadCandidates()
                    if candidates.isEmpty {
                        try await onSubmit(normalizedTitle, normalizedDescription, [])
                        dismiss()
                    } else {
                        reconciliationCandidates = candidates
                        resolvedStatesByCandidateId = [:]
                        conflictIndex = 0
                    }
                    isSubmitting = false
                } catch {
                    errorMessage = error.localizedDescription
                    isSubmitting = false
                }
            } catch {
                errorMessage = error.localizedDescription
                isSubmitting = false
            }
        }
    }

    private func submitBatch() {
        isSubmitting = true
        let reconciliations = reconciliationCandidates.map { candidate in
            ReviewDraftReconciliation(
                candidate: candidate,
                resolvedState: resolvedStatesByCandidateId[candidate.candidateId]
            )
        }
        Task {
            do {
                try await onSubmit(normalizedTitle, normalizedDescription, reconciliations)
                dismiss()
            } catch {
                errorMessage = error.localizedDescription
                isSubmitting = false
            }
        }
    }
}

struct MemoryDirectoryRenameChange: Hashable, Sendable {
    let item: MemoryListItem
    let newPath: String
}

struct MemoryDirectoryRenamePlan: Hashable, Sendable {
    let changes: [MemoryDirectoryRenameChange]
}

struct MemoryDirectoryDeletionPlan: Hashable, Sendable {
    let itemsToDelete: [MemoryListItem]
    let draftsToDiscard: [LocalDraft]
}

enum MemoryDirectoryMutationError: LocalizedError, Equatable {
    case invalidDirectory
    case invalidName
    case readOnly
    case pathCollision(String)

    var errorDescription: String? {
        switch self {
        case .invalidDirectory:
            "This folder no longer contains any memory."
        case .invalidName:
            "Choose a different folder name without a slash."
        case .readOnly:
            "Every memory in the folder must be editable before the folder can be changed."
        case .pathCollision(let path):
            "The folder cannot be renamed because \(path) already exists."
        }
    }
}

/// Pure classification of file-tree context menu operations (design v2).
///
/// Menu = generic document operations (standard macOS conventions) + domain
/// operations (Memory project membership and drafts). Add to Project exists
/// only in the read-only Org view; Draft proposals and Remove from Project
/// exist only inside an explicit Project context.
enum MemoryFileTreeMenu {
    static func isReviewSelectionReady(_ drafts: [LocalDraft]) -> Bool {
        // The request sheet reconciles behind drafts, including conflicts.
        !drafts.isEmpty && drafts.allSatisfy {
            $0.syncStatus == .synced && $0.serverId != nil
        }
    }

    /// One directory Review contains every open Organization Draft below the
    /// selection. Unchanged files and legacy Project authority are excluded.
    static func reviewableDrafts(
        _ items: [MemoryListItem],
        inOrgView: Bool
    ) -> [LocalDraft] {
        guard !inOrgView else { return [] }
        return items.compactMap(\.draft).filter {
            $0.status == .open && WorkspaceStore.canRequestReview($0)
        }
    }

    static func discardableDrafts(
        _ items: [MemoryListItem],
        inOrgView: Bool
    ) -> [LocalDraft] {
        guard !inOrgView else { return [] }
        var seen = Set<String>()
        return items.compactMap(\.draft).filter {
            $0.status != .discarded
                && $0.status != .merged
                && seen.insert($0.id).inserted
        }
            .sorted {
                $0.document.path.localizedStandardCompare($1.document.path)
                    == .orderedAscending
            }
    }

    /// Rename is an organization-authority proposal. Project views only
    /// expose it for Org resources that are still selected by that project.
    /// The Org overview is authority-only/read-only because it has no
    /// unambiguous Project carrier for a LocalDraft.
    /// Pure create Drafts keep their full local document and can be renamed;
    /// target-backed orphan rows and legacy Project authority stay read-only.
    static func canRename(_ item: MemoryListItem, inOrgView: Bool) -> Bool {
        guard !inOrgView, item.draft?.isDeletion != true else {
            return false
        }
        if item.resource?.scope == .org { return item.inherited }
        return item.resource == nil
            && item.draft?.scope == .org
            && item.draft?.targetId == nil
    }

    static func directoryRenamePlan(
        directoryId: String,
        newName: String,
        items: [MemoryListItem],
        occupiedPaths: Set<String>,
        occupiedTreePaths: Set<String>,
        inOrgView: Bool
    ) throws -> MemoryDirectoryRenamePlan {
        let name = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name != ".", name != "..", !name.contains("/") else {
            throw MemoryDirectoryMutationError.invalidName
        }
        guard let sourceDirectory = FileTreeNode.directoryPath(from: directoryId),
              !items.isEmpty else {
            throw MemoryDirectoryMutationError.invalidDirectory
        }
        guard items.allSatisfy({ canRename($0, inOrgView: inOrgView) }) else {
            throw MemoryDirectoryMutationError.readOnly
        }

        let parent = sourceDirectory.split(separator: "/").dropLast().joined(separator: "/")
        let destinationDirectory = parent.isEmpty ? name : "\(parent)/\(name)"
        guard destinationDirectory != sourceDirectory else {
            throw MemoryDirectoryMutationError.invalidName
        }

        let sourcePaths = Set(items.flatMap { item in
            [item.resource?.document.path, item.draft?.document.path].compactMap { $0 }
        }.map { $0.lowercased() })
        let sourceTreePaths = Set(items.flatMap { item in
            [item.resource?.document.path, item.draft?.document.path]
                .compactMap { $0 }
                .map { FileTreeNode.treePath(for: $0, kind: item.kind) }
        }.map { $0.lowercased() })
        let externalPaths = Set(occupiedPaths.map { $0.lowercased() })
            .subtracting(sourcePaths)
        let externalTreePaths = Set(occupiedTreePaths.map { $0.lowercased() })
            .subtracting(sourceTreePaths)
        var destinationPaths = Set<String>()
        var destinationTreePaths = Set<String>()
        var changes: [MemoryDirectoryRenameChange] = []

        for item in items {
            let treePath = FileTreeNode.treePath(for: item)
            let sourcePrefix = sourceDirectory + "/"
            guard treePath.hasPrefix(sourcePrefix) else {
                throw MemoryDirectoryMutationError.invalidDirectory
            }
            let relativePath = String(treePath.dropFirst(sourcePrefix.count))
            let destinationTreePath = destinationDirectory + "/" + relativePath
            let destinationPath = FileTreeNode.documentPath(
                fromTreePath: destinationTreePath,
                for: item
            )
            let destinationRoot = FileTreeNode.documentPath(
                fromTreePath: destinationDirectory,
                for: item
            )
            let normalizedDestinationRoot = destinationRoot.lowercased()
            let normalizedDestinationDirectory = destinationDirectory.lowercased()
            if containsPathConflict(externalPaths, at: normalizedDestinationRoot)
                || containsPathConflict(
                    externalTreePaths,
                    at: normalizedDestinationDirectory
                )
                || !destinationPaths.insert(destinationPath.lowercased()).inserted
                || !destinationTreePaths.insert(destinationTreePath.lowercased()).inserted {
                throw MemoryDirectoryMutationError.pathCollision(destinationPath)
            }
            changes.append(.init(item: item, newPath: destinationPath))
        }

        return .init(changes: changes.sorted {
            $0.item.document.path.localizedStandardCompare($1.item.document.path)
                == .orderedAscending
        })
    }

    private static func containsPathConflict(_ paths: Set<String>, at root: String) -> Bool {
        let prefix = root + "/"
        return paths.contains {
            $0 == root || $0.hasPrefix(prefix) || root.hasPrefix($0 + "/")
        }
    }

    static func directoryDeletionPlan(
        _ items: [MemoryListItem],
        inOrgView: Bool
    ) -> MemoryDirectoryDeletionPlan? {
        guard !inOrgView, !items.isEmpty else { return nil }
        var itemsToDelete: [MemoryListItem] = []
        var draftsToDiscard: [LocalDraft] = []
        for item in items {
            if canProposeOrganizationDeletion(item, inOrgView: inOrgView) {
                itemsToDelete.append(item)
            } else if item.resource == nil, let draft = item.draft {
                draftsToDiscard.append(draft)
            } else if item.draft?.isDeletion == true {
                continue
            } else {
                return nil
            }
        }
        guard !itemsToDelete.isEmpty || !draftsToDiscard.isEmpty else { return nil }
        return .init(
            itemsToDelete: itemsToDelete,
            draftsToDiscard: draftsToDiscard
        )
    }

    /// New memories are Project-bound proposals for Org authority. The Org
    /// overview is read-only and therefore has no creation scope.
    static func creationScope(inOrgView: Bool) -> MemoryScope? {
        inOrgView ? nil : .org
    }

    /// Org memories that may be added to a project: only in the Org view.
    static func addable(_ items: [MemoryListItem], inOrgView: Bool) -> [MemoryListItem] {
        inOrgView ? items.filter {
            $0.scope == .org && $0.resource != nil && $0.draft?.isDeletion != true
        } : []
    }

    /// Items inherited by the active project that may be removed from it:
    /// only in the Project view.
    static func removable(_ items: [MemoryListItem], inOrgView: Bool) -> [MemoryListItem] {
        inOrgView ? [] : items.filter(\.inherited)
    }

    /// Items that may propose deletion of Org authority. This is the shared
    /// predicate for single-row, batch, and document-toolbar actions.
    static func canProposeOrganizationDeletion(
        _ item: MemoryListItem,
        inOrgView: Bool
    ) -> Bool {
        guard item.resource?.scope == .org,
              item.draft?.isDeletion != true else {
            return false
        }
        return !inOrgView && item.inherited
    }

    static func trashable(_ items: [MemoryListItem], inOrgView: Bool) -> [MemoryListItem] {
        items.filter { canProposeOrganizationDeletion($0, inOrgView: inOrgView) }
    }
}
