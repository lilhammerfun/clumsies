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
                ContentLoadingView(title: "Loading Memory…")
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
