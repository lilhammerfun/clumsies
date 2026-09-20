import Combine
import Foundation

private struct LocalInboxReceipt: Codable {
    var revision: String
    var occurredAt: Date
    var readRevision: String?
    var archivedRevision: String?
}

@MainActor
final class InboxStore: ObservableObject {
    @Published private(set) var items: [InboxItem] = []
    @Published private(set) var isLoading = false
    @Published private(set) var hasLoaded = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var receiptError: String?
    @Published private(set) var updatingIds: Set<String> = []
    @Published private(set) var isShowingSavedContent = false

    private let context: WorkspaceContext
    private let catalog: MemoryCatalog?
    private let defaults: UserDefaults
    private let fetchPage: @MainActor (String?) async throws -> (InboxPage, Bool)
    private let fetchLocal: @MainActor () async throws -> DaemonSyncStatus
    private let updateReceipt: @MainActor (String, InboxReceiptRequest) async throws -> Void
    private var remoteItems: [InboxItem] = []
    private var localItems: [InboxItem] = []
    private var localReceipts: [String: LocalInboxReceipt] = [:]
    private var preferenceKey: String?
    private var generation = UUID()
    private var receiptGeneration = UUID()
    private var observations: Set<AnyCancellable> = []

    init(
        context: WorkspaceContext,
        catalog: MemoryCatalog? = nil,
        defaults: UserDefaults = .standard,
        fetchPage: (@MainActor (String?) async throws -> (InboxPage, Bool))? = nil,
        fetchLocal: (@MainActor () async throws -> DaemonSyncStatus)? = nil,
        updateReceipt: (@MainActor (String, InboxReceiptRequest) async throws -> Void)? = nil
    ) {
        self.context = context
        self.catalog = catalog
        self.defaults = defaults
        self.fetchPage = fetchPage ?? { cursor in
            let result: (value: InboxPage, response: DaemonServerResponse) = try await context.server.getWithMetadata(
                "/api/v1/me/inbox", query: cursor.map { [URLQueryItem(name: "cursor", value: $0)] } ?? []
            )
            return (result.value, result.response.isStaleCache)
        }
        self.fetchLocal = fetchLocal ?? {
            try await context.daemon.syncStatus()
        }
        self.updateReceipt = updateReceipt ?? { id, request in
            struct ReceiptResponse: Decodable, Sendable { let updated: Bool }
            let _: ReceiptResponse = try await context.server.send(
                method: "PATCH", path: "/api/v1/me/inbox/\(id)", body: request
            )
        }
        context.$authorityGeneration.dropFirst().sink { [weak self] _ in self?.reset() }.store(in: &observations)
        context.$projects.sink { [weak self] projects in
            self?.retainProjects(Set(projects.map(\.id)))
        }.store(in: &observations)
    }

    var unreadCount: Int { items.filter { !$0.isRead && !$0.isArchived }.count }

    func prepare(serverURL: String) {
        guard let user = context.account?.userId, let org = context.organization?.orgId else { return }
        let key = "ClumsiesInbox.\(serverURL).\(org).\(user)"
        guard preferenceKey != key else { return }
        reset()
        preferenceKey = key
        if let data = defaults.data(forKey: key),
           let receipts = try? JSONDecoder().decode([String: LocalInboxReceipt].self, from: data) {
            localReceipts = receipts
        }
    }

    func reset() {
        generation = UUID()
        preferenceKey = nil
        remoteItems = []
        localItems = []
        items = []
        localReceipts = [:]
        isLoading = false
        hasLoaded = false
        errorMessage = nil
        receiptError = nil
        isShowingSavedContent = false
        updatingIds = []
    }

    func refresh() async {
        guard context.phase == .ready, preferenceKey != nil, !isLoading else { return }
        isLoading = true
        let requestGeneration = generation
        let authority = context.authorityGeneration
        let initialReceipts = receiptGeneration
        defer { if generation == requestGeneration { isLoading = false; hasLoaded = true } }
        var errors: [String] = []
        // ponytail: reload keyset pages; use an incremental feed if inbox history becomes expensive.
        do {
            var loaded: [InboxItem] = []
            var cursor: String?
            var seenCursors: Set<String> = []
            var saved = false
            repeat {
                let (page, stale) = try await fetchPage(cursor)
                try context.ensureAuthority(authority)
                guard generation == requestGeneration else { return }
                saved = saved || stale
                loaded += page.items.map(InboxItem.server)
                cursor = page.nextCursor
                if let cursor, page.items.isEmpty || !seenCursors.insert(cursor).inserted {
                    throw DaemonXPCError.invalidReply
                }
            } while cursor != nil
            // A fallback cache may predate a receipt already confirmed in this process.
            let current = Dictionary(uniqueKeysWithValues: remoteItems.map { ($0.id, $0) })
            remoteItems = loaded.map { incoming in
                guard let previous = current[incoming.id],
                      (saved || updatingIds.contains(incoming.id) || initialReceipts != receiptGeneration),
                      (previous.serverVersion ?? 0) >= (incoming.serverVersion ?? 0) else { return incoming }
                return previous
            }
            isShowingSavedContent = saved
        } catch is CancellationError { return }
        catch {
            guard context.authorityGeneration == authority, generation == requestGeneration else { return }
            errors.append(String(localized: "Couldn't refresh team notifications. \(error.localizedDescription)"))
        }
        do {
            let snapshot = try await fetchLocal()
            try context.ensureAuthority(authority)
            guard generation == requestGeneration else { return }
            installLocal(Self.localNotifications(snapshot) + localSharedNotifications())
        } catch is CancellationError { return }
        catch {
            guard context.authorityGeneration == authority, generation == requestGeneration else { return }
            errors.append(String(localized: "Couldn't check this Mac. \(error.localizedDescription)"))
            installLocal(localItems.filter { $0.id != "local:unavailable" } + [.init(
                id: "local:unavailable", type: .syncErrors, projectId: nil, projectName: String(localized: "This Mac"),
                title: String(localized: "Sync status couldn't be checked"), message: String(localized: "Reconnect to the local service, then refresh Inbox."),
                occurredAt: .distantPast, needsAction: true, revision: "unavailable",
                isRead: false, isArchived: false, destination: .retrySync
            )])
        }
        guard generation == requestGeneration, context.authorityGeneration == authority else { return }
        errorMessage = errors.isEmpty ? nil : errors.joined(separator: "\n")
        retainProjects(Set(context.projects.map(\.id)))
    }

    /// Only successful navigation or recovery acknowledges the displayed revision.
    func activate(
        _ item: InboxItem,
        open: @MainActor (InboxDestination) async throws -> Void
    ) async throws {
        guard let destination = item.destination else { return }
        try Task.checkCancellation()
        guard items.contains(where: { $0.id == item.id && $0.revision == item.revision }) else {
            throw CancellationError()
        }
        let requestGeneration = generation
        let authority = context.authorityGeneration
        try await open(destination)
        // Leaving Inbox cancels its view task; finish the successful navigation's receipt.
        await Task {
            guard self.generation == requestGeneration, self.context.authorityGeneration == authority else { return }
            await self.acknowledge(item, action: .read)
        }.value
    }

    @discardableResult
    func acknowledge(_ item: InboxItem, action: InboxReceiptAction) async -> Bool {
        guard !updatingIds.contains(item.id), preferenceKey != nil,
              items.contains(where: { $0.id == item.id && $0.revision == item.revision }) else { return false }
        let requestGeneration = generation
        let authority = context.authorityGeneration
        receiptError = nil
        updatingIds.insert(item.id)
        defer { if generation == requestGeneration { updatingIds.remove(item.id) } }
        if let version = item.serverVersion {
            do {
                try await updateReceipt(item.id, .init(version: version, action: action))
                try context.ensureAuthority(authority)
                guard generation == requestGeneration else { return false }
                receiptGeneration = UUID()
                if let index = remoteItems.firstIndex(where: { $0.id == item.id && $0.revision == item.revision }) {
                    Self.apply(action, to: &remoteItems[index])
                }
            } catch is CancellationError { return false }
            catch {
                guard context.authorityGeneration == authority, generation == requestGeneration else { return false }
                receiptError = String(localized: "Couldn't update this notification. \(error.localizedDescription)")
                return false
            }
        } else if var receipt = localReceipts[item.id], receipt.revision == item.revision {
            switch action {
            case .read: receipt.readRevision = item.revision
            case .unread: receipt.readRevision = nil
            case .archive: receipt.archivedRevision = item.revision
            case .restore: receipt.archivedRevision = nil
            }
            localReceipts[item.id] = receipt
            if let index = localItems.firstIndex(where: { $0.id == item.id && $0.revision == item.revision }) {
                Self.apply(action, to: &localItems[index])
            }
            saveReceipts()
        }
        publishItems()
        return true
    }

    func acknowledge(_ selection: [InboxItem], action: InboxReceiptAction, undoManager: UndoManager?) async {
        let requestGeneration = generation
        var changed: [InboxItem] = []
        for item in selection {
            guard generation == requestGeneration else { return }
            guard await acknowledge(item, action: action) else { break }
            changed.append(item)
        }
        guard generation == requestGeneration, !changed.isEmpty,
              action == .archive || action == .restore else { return }
        let archived = changed
        undoManager?.registerUndo(withTarget: self) { store in
            Task { @MainActor in
                for item in archived {
                    guard store.generation == requestGeneration else { return }
                    guard await store.acknowledge(item, action: action == .archive ? .restore : .archive) else { break }
                }
            }
        }
        undoManager?.setActionName(action == .archive ? String(localized: "Archive Notifications") : String(localized: "Move to Inbox"))
    }

    private static func apply(_ action: InboxReceiptAction, to item: inout InboxItem) {
        switch action {
        case .read: item.isRead = true
        case .unread: item.isRead = false
        case .archive: item.isArchived = true
        case .restore: item.isArchived = false
        }
    }

    private func installLocal(_ notices: [InboxItem]) {
        localItems = notices.map { notice in
            var notice = notice
            var receipt = localReceipts[notice.id] ?? .init(revision: notice.revision, occurredAt: Date())
            if receipt.revision != notice.revision { receipt.revision = notice.revision; receipt.occurredAt = Date() }
            notice = .init(id: notice.id, type: notice.type, projectId: notice.projectId, projectName: notice.projectName,
                title: notice.title, message: notice.message, occurredAt: receipt.occurredAt,
                needsAction: notice.needsAction, revision: notice.revision,
                isRead: receipt.readRevision == notice.revision, isArchived: receipt.archivedRevision == notice.revision,
                destination: notice.destination)
            localReceipts[notice.id] = receipt
            return notice
        }
        // Resolved conditions leave the local inbox; a later recurrence is a new reminder.
        let activeIds = Set(notices.map(\.id))
        localReceipts = localReceipts.filter { activeIds.contains($0.key) }
        saveReceipts()
    }

    private func saveReceipts() {
        guard let preferenceKey else { return }
        do { defaults.set(try JSONEncoder().encode(localReceipts), forKey: preferenceKey) }
        catch { receiptError = String(localized: "Couldn't save notification preferences. \(error.localizedDescription)") }
    }

    private func retainProjects(_ accessible: Set<String>) {
        remoteItems.removeAll { $0.projectId.map { !accessible.contains($0) } == true }
        localItems.removeAll { $0.projectId.map { !accessible.contains($0) } == true }
        publishItems()
    }

    private func publishItems() {
        items = (remoteItems + localItems).sorted {
            if $0.occurredAt != $1.occurredAt { return $0.occurredAt > $1.occurredAt }
            return $0.id < $1.id
        }
    }

    private func localSharedNotifications() -> [InboxItem] {
        guard let catalog else { return [] }
        // Cover pre-Inbox updates and changes from this user's other devices.
        // A persisted server subject already provides the same project's entry.
        let projects = Dictionary(uniqueKeysWithValues: context.projects.map { ($0.id, $0.name) })
        return Dictionary(grouping: catalog.staleResourceSnapshots, by: { $0.value.projectId }).compactMap { projectId, snapshots in
            guard let name = projects[projectId], !remoteItems.contains(where: { $0.id == "shared:\(projectId)" }) else { return nil }
            let revision = snapshots.map { "\($0.key):\($0.value.authoritativeCommitId)" }.sorted().joined(separator: "|")
            return .init(id: "local:shared:\(projectId)", type: .sharedUpdates, projectId: projectId, projectName: name,
                title: String(localized: "Remote Memory updated"), message: String(localized: "Compare the remote version with the files on this Mac."),
                occurredAt: .distantPast, needsAction: false, revision: revision, isRead: false, isArchived: false,
                destination: .sharedChanges(projectId: projectId))
        }
    }

    static func localNotifications(_ sync: DaemonSyncStatus) -> [InboxItem] {
        var notices: [InboxItem] = []
        if sync.failedOperationCount > 0 || [sync.draftSync.state, sync.commitSync.state].contains(where: { ["failed", "degraded"].contains($0) }) {
            notices.append(.init(id: "local:sync", type: .syncErrors, projectId: nil, projectName: String(localized: "This Mac"),
                title: String(localized: "Changes couldn't sync"),
                message: sync.draftSync.lastError?.message ?? sync.commitSync.lastError?.message
                    ?? String(localized: "Some changes haven't reached the server. Retry when connected."),
                occurredAt: .distantPast, needsAction: true, revision: "sync-failed", isRead: false, isArchived: false,
                destination: .retrySync))
        }
        return notices
    }
}
