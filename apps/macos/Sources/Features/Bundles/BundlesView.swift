import SwiftUI

struct BundleNavigator: View {
    let store: WorkspaceCoordinator
    @EnvironmentObject private var bundleModel: BundlesModel
    @EnvironmentObject private var bundleStore: BundleStore
    @EnvironmentObject private var workspaceNavigation: WorkspaceNavigation

    var body: some View {
        Group {
            if bundleStore.bundles.isEmpty {
                BundleCollectionStatusView(store: store)
            } else if !query.isEmpty, bundles.isEmpty {
                ContentUnavailableView.search(text: query)
            } else {
                List(selection: $bundleModel.selectedBundleId) {
                    ForEach(bundles) { bundle in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(bundle.name)
                                .lineLimit(1)
                            Text("\(bundle.resourceIds.count) resources")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .tag(bundle.id)
                        .contextMenu {
                            Button("Delete", role: .destructive) {
                                Task { await bundleModel.deleteBundle(bundle) }
                            }
                        }
                    }
                }
                .listStyle(.inset)
                .safeAreaInset(edge: .bottom) {
                    BundleCollectionStatusBanner(store: store)
                }
            }
        }
        .task { await store.prepareWorkspaceIndex(includeContent: false) }
    }

    private var query: String {
        workspaceNavigation.searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var bundles: [PersonalBundle] {
        BundleStore.filterBundles(bundleStore.bundles, query: query)
    }
}

struct BundleDetail: View {
    @EnvironmentObject private var bundleStore: BundleStore
    let store: WorkspaceCoordinator
    @EnvironmentObject private var bundleModel: BundlesModel
    @Binding var showsResourcePicker: Bool
    @Binding var confirmsDeletion: Bool

    var body: some View {
        if let bundle = bundleModel.selectedBundle {
            BundleEditor(
                store: store,
                bundle: bundle,
                showsResourcePicker: $showsResourcePicker,
                confirmsDeletion: $confirmsDeletion
            )
                .id(bundle.id)
        } else {
            BundleCollectionStatusView(store: store)
        }
    }
}

private struct BundleCollectionStatusView: View {
    let store: WorkspaceCoordinator
    @EnvironmentObject private var bundleStore: BundleStore

    var body: some View {
        switch bundleStore.bundleLoadState {
        case .loading:
            ContentLoadingView(title: "Loading Bundles…")
        case .failed(let message):
            ContentUnavailableView {
                Label("Bundles Unavailable", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                Button("Try Again") { Task { await store.reload() } }
            }
        case .loaded:
            ContentUnavailableView(
                "No Bundles",
                systemImage: "shippingbox",
                description: Text("Create a personal Bundle to collect memory for recurring work.")
            )
        }
    }
}

private struct BundleCollectionStatusBanner: View {
    let store: WorkspaceCoordinator
    @EnvironmentObject private var bundleStore: BundleStore

    @ViewBuilder
    var body: some View {
        switch bundleStore.bundleLoadState {
        case .loading:
            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                Text("Refreshing Bundles...")
                    .font(.caption)
            }
            .padding(8)
            .frame(maxWidth: .infinity)
            .background(.bar)
        case .failed:
            Button("Bundle refresh failed - Try Again") {
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

private struct BundleEditor: View {
    let store: WorkspaceCoordinator
    @EnvironmentObject private var bundleModel: BundlesModel
    @EnvironmentObject private var bundleStore: BundleStore
    @EnvironmentObject private var memoryCatalog: MemoryCatalog
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var workspaceFeedback: WorkspaceFeedback
    let bundle: PersonalBundle

    @State private var name: String
    @State private var description: String
    @State private var resourceIds: Set<String>
    @Binding var showsResourcePicker: Bool
    @Binding var confirmsDeletion: Bool
    @State private var isDeleting = false

    init(
        store: WorkspaceCoordinator,
        bundle: PersonalBundle,
        showsResourcePicker: Binding<Bool>,
        confirmsDeletion: Binding<Bool>
    ) {
        self.store = store
        self.bundle = bundle
        _name = State(initialValue: bundle.name)
        _description = State(initialValue: bundle.description)
        _resourceIds = State(initialValue: Set(bundle.resourceIds))
        _showsResourcePicker = showsResourcePicker
        _confirmsDeletion = confirmsDeletion
    }

    var body: some View {
        Form {
            Section("Bundle") {
                TextField("Name", text: $name)
                TextField("Description", text: $description, axis: .vertical)
                    .lineLimit(2...6)
            }
            Section {
                if selectedResources.isEmpty {
                    Text("No memory has been added.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(selectedResources) { resource in
                        HStack(spacing: 10) {
                            Button {
                                open(resource)
                            } label: {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(resource.document.title)
                                        .lineLimit(1)
                                    Text(resourceLocation(resource))
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .focusEffectDisabled()
                            .help("Open \(resource.document.title)")
                            .accessibilityLabel("Open \(resource.document.title)")

                            Button {
                                resourceIds.remove(resource.id)
                            } label: {
                                Image(systemName: "minus.circle")
                            }
                            .buttonStyle(.borderless)
                            .help("Remove from Bundle")
                        }
                    }
                }
            } header: {
                HStack {
                    Text("Memory")
                    Spacer()
                    Text(resourceIds.count, format: .number)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .disabled(workspaceContext.phase != .ready)
        .sheet(isPresented: $showsResourcePicker) {
            BundleResourcePicker(resources: selectableResources, selection: $resourceIds)
        }
        .confirmationDialog(
            "Delete \(bundle.name)?",
            isPresented: $confirmsDeletion,
            titleVisibility: .visible
        ) {
            Button("Delete Bundle", role: .destructive) { deleteBundle() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The Bundle will be permanently removed. Its memory is not affected.")
        }
        .onChange(of: name) { _, _ in scheduleSave() }
        .onChange(of: description) { _, _ in scheduleSave() }
        .onChange(of: resourceIds) { _, _ in scheduleSave() }
        .onDisappear { flushSave() }
    }

    private var selectableResources: [MemoryResource] {
        memoryCatalog.resources.filter { $0.scope == .org }
    }

    private var selectedResources: [MemoryResource] {
        selectableResources
            .filter { resourceIds.contains($0.id) }
            .sorted {
                let locationOrder = resourceLocation($0).localizedStandardCompare(resourceLocation($1))
                if locationOrder != .orderedSame { return locationOrder == .orderedAscending }
                return $0.document.title.localizedStandardCompare($1.document.title) == .orderedAscending
            }
    }

    private var hasChanges: Bool {
        name != bundle.name
            || description != bundle.description
            || resourceIds != Set(bundle.resourceIds)
    }

    private func resourceLocation(_ resource: MemoryResource) -> String {
        let scope = resource.scope == .org ? "Organization" : resource.projectName ?? "Project"
        return "\(scope) · \(resource.kind.singularTitle)"
    }

    private func open(_ resource: MemoryResource) {
        let item = MemoryListItem(
            id: resource.id,
            resource: resource,
            draft: nil,
            inherited: false
        )
        Task { await store.reveal(item) }
    }

    private func scheduleSave() {
        guard !isDeleting else { return }
        bundleStore.stageBundleSave(
            bundle,
            name: name,
            description: description,
            resourceIds: resourceIds
        )
    }

    private func flushSave() {
        guard !isDeleting, hasChanges else { return }
        Task {
            do {
                try await bundleStore.flushBundleSave(bundle.id)
            } catch {
                workspaceFeedback.errorMessage = error.localizedDescription
            }
        }
    }

    private func deleteBundle() {
        isDeleting = true
        bundleStore.cancelBundleSave(bundle.id)
        Task { await bundleModel.deleteBundle(bundle) }
    }
}

private struct BundleResourcePicker: View {
    let resources: [MemoryResource]
    @Binding var selection: Set<String>

    @Environment(\.dismiss) private var dismiss
    @State private var query = ""

    var body: some View {
        NavigationStack {
            List {
                ForEach(MemoryKind.allCases) { kind in
                    let candidates = filteredResources.filter { $0.kind == kind }
                    if !candidates.isEmpty {
                        Section(kind.title) {
                            ForEach(candidates) { resource in
                                Toggle(isOn: selectionBinding(for: resource.id)) {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(resource.document.title)
                                            .lineLimit(1)
                                        Text(resourceLocation(resource))
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                }
                                .accessibilityLabel(
                                    "\(resource.document.title), \(resourceLocation(resource))"
                                )
                            }
                        }
                    }
                }
            }
            .searchable(text: $query, prompt: "Search memory")
            .navigationTitle("Add Memory")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .keyboardShortcut(.defaultAction)
                }
            }
        }
        .frame(width: 620, height: 600)
    }

    private var filteredResources: [MemoryResource] {
        let candidates = resources.sorted {
            $0.document.title.localizedStandardCompare($1.document.title) == .orderedAscending
        }
        let term = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !term.isEmpty else { return candidates }
        return candidates.filter {
            $0.document.title.localizedCaseInsensitiveContains(term)
                || $0.document.path.localizedCaseInsensitiveContains(term)
                || resourceLocation($0).localizedCaseInsensitiveContains(term)
        }
    }

    private func selectionBinding(for resourceId: String) -> Binding<Bool> {
        Binding(
            get: { selection.contains(resourceId) },
            set: { selected in
                if selected { selection.insert(resourceId) }
                else { selection.remove(resourceId) }
            }
        )
    }

    private func resourceLocation(_ resource: MemoryResource) -> String {
        resource.scope == .org ? "Organization" : resource.projectName ?? "Project"
    }
}
