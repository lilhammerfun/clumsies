import SwiftUI

private enum InboxFilter: String, CaseIterable, Identifiable {
    case inbox = "Inbox", unread = "Unread", archived = "Archived"
    var id: Self { self }

    var title: String {
        switch self {
        case .inbox: String(localized: "Inbox")
        case .unread: String(localized: "Unread")
        case .archived: String(localized: "Archived")
        }
    }
}

struct InboxView: View {
    @Environment(\.undoManager) private var undoManager
    @ObservedObject var store: InboxStore
    let searchFocusToken: UUID
    let open: @MainActor (InboxDestination) async throws -> Void
    @State private var filter: InboxFilter = .inbox
    @State private var messageType: InboxMessageType?
    @State private var query = ""
    @State private var searchFocusRequest = 0
    @State private var selection: Set<String> = []
    @State private var actionError: String?
    @State private var openingId: String?
    @State private var message: InboxItem?
    @State private var actionTask: Task<Void, Never>?

    private var visibleItems: [InboxItem] {
        store.items.filter { item in
            (messageType == nil || item.type == messageType)
                && (filter == .archived ? item.isArchived : !item.isArchived)
                && (filter != .unread || !item.isRead || selection == [item.id])
                && (query.isEmpty || "\(item.title) \(item.summary)".localizedStandardContains(query))
        }
    }

    private var selectedItems: [InboxItem] { visibleItems.filter { selection.contains($0.id) } }
    private var isBusy: Bool { openingId != nil || !store.updatingIds.isEmpty }
    private var readAction: InboxReceiptAction { selectedItems.contains { !$0.isRead } ? .read : .unread }

    var body: some View {
        VStack(spacing: 0) {
            if store.isShowingSavedContent {
                Text("Showing saved notifications. Connect to refresh or update read status.")
                    .font(.callout).foregroundStyle(.secondary).padding(.horizontal, 12)
            }
            if let error = actionError ?? store.receiptError ?? store.errorMessage {
                HStack(alignment: .top) {
                    Text(error).font(.callout).textSelection(.enabled)
                    Spacer()
                    Button("Refresh") {
                        actionError = nil
                        Task { await store.refresh() }
                    }
                    .disabled(store.isLoading || isBusy)
                }
                .padding(12)
            }
            notificationList
        }
        .sheet(item: $message) { item in
            InboxMessageView(item: item, projectId: store.welcomeProjectId, receiptError: store.receiptError, open: open)
                .task { await store.acknowledge(item, action: .read) }
        }
        .onChange(of: store.items.map(\.id)) { _, ids in
            if let message, !ids.contains(message.id) { self.message = nil }
        }
        .navigationTitle("Inbox")
        .toolbar { toolbarContent }
        .onChange(of: visibleItems.map(\.id)) { _, ids in selection.formIntersection(ids) }
        .onChange(of: selection) { _, ids in
            if ids.count == 1, let item = visibleItems.first(where: { ids.contains($0.id) }) {
                markRead(item)
            }
        }
        .onChange(of: searchFocusToken) { _, _ in searchFocusRequest += 1 }
        .task { await store.refresh() }
        .onDisappear { actionTask?.cancel() }
    }

    private var notificationList: some View {
        List(selection: $selection) {
            ForEach(visibleItems) { item in
                InboxRow(item: item, isOpening: openingId == item.id, isActionDisabled: openingId != nil) { activate(item) }
                    .tag(item.id)
            }
        }
        .listStyle(.inset)
        .contextMenu(forSelectionType: String.self) { ids in
            contextActions(for: visibleItems.filter { ids.contains($0.id) })
        } primaryAction: { ids in
            if ids.count == 1, let item = visibleItems.first(where: { ids.contains($0.id) }) {
                activate(item)
            }
        }
        .overlay {
            if visibleItems.isEmpty {
                if store.isLoading && !store.hasLoaded {
                    ProgressView("Loading Inbox")
                } else {
                    ContentUnavailableView(
                        query.isEmpty ? "No Notifications" : "No Results",
                        systemImage: "tray",
                        description: Text(filter == .unread ? "You're up to date." : "Notifications about your team, Memory, and Reviews appear here.")
                    )
                }
            }
        }
    }

    private func markRead(_ item: InboxItem) {
        guard !item.isRead else { return }
        Task { await store.acknowledge(item, action: .read) }
    }

    private func activate(_ item: InboxItem) {
        guard openingId == nil, item.actionTitle != nil else { return }
        if item.body != nil {
            message = item
            return
        }
        actionError = nil
        openingId = item.id
        actionTask = Task {
            defer { openingId = nil }
            do {
                try await store.activate(item, open: open)
            } catch is CancellationError { }
            catch { if !Task.isCancelled { actionError = error.localizedDescription } }
        }
    }

    private func update(_ items: [InboxItem], action: InboxReceiptAction) {
        guard !isBusy else { return }
        Task { await store.acknowledge(items, action: action, undoManager: undoManager) }
    }

    @ViewBuilder
    private func contextActions(for items: [InboxItem]) -> some View {
        if items.count == 1, let item = items.first, let title = item.actionTitle {
            Button(title) { activate(item) }.disabled(openingId != nil)
            Divider()
        }
        if !items.isEmpty {
            if items.contains(where: { !$0.isRead }) {
                Button("Mark as Read", systemImage: "envelope.open") { update(items, action: .read) }.disabled(isBusy)
            }
            if items.contains(where: \.isRead) {
                Button("Mark as Unread", systemImage: "envelope.badge") { update(items, action: .unread) }.disabled(isBusy)
            }
            Divider()
            Button(filter == .archived ? "Move to Inbox" : "Archive", systemImage: filter == .archived ? "tray.and.arrow.up" : "archivebox") {
                update(items, action: filter == .archived ? .restore : .archive)
            }
            .disabled(isBusy)
        }
    }

    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        ToolbarItem(id: "inbox.filter", placement: .navigation) {
            ToolbarFilterMenu(selectionTitle: filter.title) {
                ForEach(InboxFilter.allCases) { option in
                    Toggle(option.title, isOn: Binding(
                        get: { filter == option },
                        set: { if $0 { filter = option } }
                    ))
                }
            }
            .toolbarHelp(String(localized: "Filter Inbox"))
            .accessibilityLabel("Filter Inbox")
            .accessibilityValue(filter.title)
            .accessibilityIdentifier("inbox-toolbar-filter")
        }
        ToolbarItem(id: "inbox.type", placement: .navigation) {
            ToolbarFilterMenu(selectionTitle: messageType?.title ?? String(localized: "All Types")) {
                Toggle("All Types", isOn: Binding(
                    get: { messageType == nil },
                    set: { if $0 { messageType = nil } }
                ))
                Divider()
                ForEach(InboxMessageType.allCases) { type in
                    Toggle(type.title, isOn: Binding(
                        get: { messageType == type },
                        set: { if $0 { messageType = type } }
                    ))
                }
            }
            .toolbarHelp(String(localized: "Filter Inbox by Message Type"))
            .accessibilityLabel("Message Type Filter")
            .accessibilityValue(messageType?.title ?? String(localized: "All Types"))
            .accessibilityIdentifier("inbox-toolbar-type")
        }
        if #available(macOS 26.0, *) {
            ToolbarSpacer(.flexible, placement: .automatic)
        }
        ToolbarItem(id: "inbox.read", placement: .trailingPinned) {
            Button(readAction == .read ? "Mark as Read" : "Mark as Unread",
                   systemImage: readAction == .read ? "envelope.open" : "envelope.badge") {
                update(selectedItems, action: readAction)
            }
            .toolbarHelp(readAction == .read ? String(localized: "Mark as Read (⇧⌘U)") : String(localized: "Mark as Unread (⇧⌘U)"))
            .keyboardShortcut("u", modifiers: [.command, .shift])
            .accessibilityIdentifier("inbox-toolbar-read")
            .disabled(selectedItems.isEmpty || isBusy)
        }
        ToolbarItem(id: "inbox.archive", placement: .trailingPinned) {
            Button(filter == .archived ? "Move to Inbox" : "Archive",
                   systemImage: filter == .archived ? "tray.and.arrow.up" : "archivebox") {
                update(selectedItems, action: filter == .archived ? .restore : .archive)
            }
            .toolbarHelp(filter == .archived ? String(localized: "Move to Inbox") : String(localized: "Archive (⌃⌘A)"))
            .keyboardShortcut("a", modifiers: [.control, .command])
            .accessibilityIdentifier("inbox-toolbar-archive")
            .disabled(selectedItems.isEmpty || isBusy)
        }
        if #available(macOS 26.0, *) {
            ToolbarSpacer(.fixed, placement: .automatic)
        }
        ToolbarItem(id: "inbox.refresh", placement: .trailingPinned) {
            Button { Task { await store.refresh() } } label: { Image(systemName: "arrow.clockwise") }
                .toolbarHelp(String(localized: "Refresh Inbox"))
                .accessibilityLabel("Refresh Inbox")
                .accessibilityIdentifier("inbox-toolbar-refresh")
                .disabled(store.isLoading || isBusy)
        }
        if #available(macOS 26.0, *) {
            ToolbarSpacer(.fixed, placement: .automatic)
        }
        ToolbarItem(id: "inbox.search", placement: .trailingPinned) {
            ClassicSearchField(
                text: $query,
                prompt: String(localized: "Search Inbox"),
                accessibilityIdentifier: "inbox-toolbar-search",
                accessibilityHelp: String(localized: "Search notifications by title, message or project"),
                focusToken: searchFocusRequest
            )
        }
    }
}

struct InboxRow: View {
    let item: InboxItem
    let isOpening: Bool
    var isActionDisabled = false
    let activate: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Circle().fill(item.isRead ? Color.clear : Color.accentColor)
                .frame(width: 7, height: 7).padding(.top, 6)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Text(item.title).fontWeight(item.isRead ? .regular : .semibold)
                        .lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 12)
                    Text(item.occurredAt.formatted(date: .abbreviated, time: .shortened))
                        .font(.caption).foregroundStyle(.secondary).fixedSize()
                }
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Text(item.summary).foregroundStyle(.secondary).lineLimit(1)
                    Spacer(minLength: 12)
                    if let title = item.actionTitle {
                        Button(action: activate) {
                            HStack(spacing: 5) {
                                if isOpening { ProgressView().controlSize(.mini) }
                                Text(title)
                            }
                        }
                        .buttonStyle(.borderless)
                        .fixedSize()
                        .disabled(isOpening || isActionDisabled)
                        .accessibilityIdentifier("inbox-open-\(item.id)")
                    }
                }
                .font(.callout)
            }
        }
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .help("\(item.title)\n\(item.summary)")
        .accessibilityElement(children: .contain)
        .accessibilityLabel(item.isRead ? "Read notification" : "Unread notification")
        .accessibilityIdentifier("inbox-row-\(item.id)")
    }
}

struct InboxMessageView: View {
    @Environment(\.dismiss) private var dismiss
    let item: InboxItem
    let projectId: String?
    var receiptError: String?
    let open: @MainActor (InboxDestination) async throws -> Void
    @State private var error: String?
    @State private var isOpening = false
    @State private var openTask: Task<Void, Never>?

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 8) {
                Text(item.title).font(.title2.bold())
                Text("Clumsies · \(item.occurredAt.formatted(date: .abbreviated, time: .shortened))")
                    .font(.callout).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading).padding(24)
            Divider()
            MarkdownPreview(source: item.body ?? "")
            if let error = error ?? receiptError {
                Text(error).foregroundStyle(.red).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 24).padding(.vertical, 12)
            }
            SheetActionBar(
                confirmationTitle: projectId == nil ? Text("Close") : Text("Open Memory"),
                cancellationTitle: "Close", progressTitle: "Opening…", isWorking: isOpening,
                allowsCancellationWhileWorking: true,
                cancel: projectId == nil ? nil : { dismiss() }, confirm: {
                    guard let projectId else { dismiss(); return }
                    isOpening = true
                    openTask = Task {
                        defer { isOpening = false }
                        do { try await open(.project(projectId)); dismiss() }
                        catch is CancellationError { dismiss() }
                        catch { self.error = error.localizedDescription }
                    }
                }
            )
        }
        .frame(width: 640, height: 600)
        .onDisappear { openTask?.cancel() }
    }
}
