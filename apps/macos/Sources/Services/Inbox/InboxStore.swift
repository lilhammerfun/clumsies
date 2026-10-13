import Combine
import CryptoKit
import Foundation

private struct LocalInboxReceipt: Codable {
    var revision: String
    var occurredAt: Date
    var readRevision: String?
    var archivedRevision: String?
}

private struct PendingInboxReceipt: Codable, Identifiable {
    let id: UUID
    let notificationId: String
    let version: Int
    let action: InboxReceiptAction
}

private struct InboxReceiptState: Codable {
    let version: Int
    let local: [String: LocalInboxReceipt]
    let pending: [PendingInboxReceipt]
}

@MainActor
final class InboxStore: ObservableObject {
    @Published private(set) var unavailableProjects: [DaemonUnavailableProject] = []
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
    private let receiptDirectory: URL
    private let fetchPage: @MainActor (String?) async throws -> (InboxPage, Bool)
    private let fetchLocal: @MainActor () async throws -> DaemonSyncStatus
    private let updateReceipt: @MainActor (String, InboxReceiptRequest) async throws -> Void
    private var remoteItems: [InboxItem] = []
    private var localItems: [InboxItem] = []
    private var localReceipts: [String: LocalInboxReceipt] = [:]
    private var pendingReceipts: [PendingInboxReceipt] = []
    private var preferenceKey: String?
    private var receiptStorageLoaded = false
    private var hasReceiptStorageError = false
    private var generation = UUID()
    private var receiptGeneration = UUID()
    private var observations: Set<AnyCancellable> = []

    init(
        receiptDirectory: URL? = nil,
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
        self.receiptDirectory = receiptDirectory ?? ClumsiesIdentifiers.clientApplicationSupportDirectoryURL
            .appending(path: "Inbox", directoryHint: .isDirectory)
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

    var welcomeProjectId: String? {
        context.projects.first { $0.id == context.activeProjectId }?.id ?? context.projects.first?.id
    }

    var unreadCount: Int { items.filter { !$0.isRead && !$0.isArchived }.count }

    func prepare(serverURL: String) {
        guard let user = context.account?.userId, let org = context.organization?.orgId else { return }
        let key = "ClumsiesInbox.\(serverURL).\(org).\(user)"
        guard preferenceKey != key || !receiptStorageLoaded else { return }
        reset()
        preferenceKey = key
        loadReceipts()
    }

    func reset() {
        generation = UUID()
        preferenceKey = nil
        receiptStorageLoaded = false
        hasReceiptStorageError = false
        remoteItems = []
        localItems = []
        unavailableProjects = []
        items = []
        localReceipts = [:]
        pendingReceipts = []
        isLoading = false
        hasLoaded = false
        errorMessage = nil
        receiptError = nil
        isShowingSavedContent = false
        updatingIds = []
    }

    func refresh() async {
        guard context.phase == .ready, preferenceKey != nil, !isLoading else { return }
        if !receiptStorageLoaded { loadReceipts() }
        isLoading = true
        let requestGeneration = generation
        let authority = context.authorityGeneration
        // Retry only on the existing refresh cadence; a failure stops this pass.
        for pending in pendingReceipts where !updatingIds.contains(pending.notificationId) {
            _ = await sendPendingReceipt(pending)
            guard generation == requestGeneration else { return }
            if pendingReceipts.contains(where: { $0.id == pending.id }) { break }
        }
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
        } catch where error.isUserCancellation { return }
        catch {
            guard context.authorityGeneration == authority, generation == requestGeneration else { return }
            if remoteItems.isEmpty { errors.append(error.userFacingMessage) }
            isShowingSavedContent = !remoteItems.isEmpty
        }
        do {
            let snapshot = try await fetchLocal()
            try context.ensureAuthority(authority)
            guard generation == requestGeneration else { return }
            unavailableProjects = snapshot.unavailableProjects
            installLocal(Self.localNotifications(snapshot) + localSharedNotifications())
        } catch where error.isUserCancellation { return }
        catch {
            guard context.authorityGeneration == authority, generation == requestGeneration else { return }
            // A failed status probe is not a new business notification. Keep known sync state.
            if items.isEmpty && errors.isEmpty { errors.append(error.userFacingMessage) }
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
        guard receiptStorageLoaded,
              items.contains(where: { $0.id == item.id && $0.revision == item.revision }) else { return false }
        receiptError = nil
        if let version = item.serverVersion {
            let pending = PendingInboxReceipt(id: UUID(), notificationId: item.id, version: version, action: action)
            let prior = pendingReceipts
            let isReadAction = action == .read || action == .unread
            pendingReceipts.removeAll {
                $0.notificationId == item.id && $0.version == version
                    && (($0.action == .read || $0.action == .unread) == isReadAction)
            }
            pendingReceipts.append(pending)
            guard saveReceipts() else { pendingReceipts = prior; return false }
            publishItems()
            if let failure = ClientServiceStatus.shared.failure,
               [.connection, .localService, .authentication].contains(failure) {
                return true
            }
            return await sendPendingReceipt(pending)
        } else if var receipt = localReceipts[item.id], receipt.revision == item.revision {
            let prior = localReceipts
            let priorItems = localItems
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
            guard saveReceipts() else {
                localReceipts = prior
                localItems = priorItems
                return false
            }
        }
        publishItems()
        return true
    }

    func dismissReceiptError() { receiptError = nil }

    private func receiptFile(for key: String) -> URL {
        let identity = SHA256.hash(data: Data(key.utf8)).map { String(format: "%02x", $0) }.joined()
        return receiptDirectory.appending(path: identity + ".json", directoryHint: .notDirectory)
    }

    private func loadReceipts() {
        guard let preferenceKey else { return }
        do {
            let file = receiptFile(for: preferenceKey)
            let stored: InboxReceiptState
            do {
                stored = try JSONDecoder().decode(InboxReceiptState.self, from: Data(contentsOf: file))
            } catch let error as CocoaError where error.code == .fileReadNoSuchFile {
                stored = .init(version: 1,
                    local: try defaults.data(forKey: preferenceKey).map {
                        try JSONDecoder().decode([String: LocalInboxReceipt].self, from: $0)
                    } ?? [:],
                    pending: try defaults.data(forKey: preferenceKey + ".pending").map {
                        try JSONDecoder().decode([PendingInboxReceipt].self, from: $0)
                    } ?? [])
            }
            guard stored.version == 1 else { throw CocoaError(.coderReadCorrupt) }
            localReceipts = stored.local
            pendingReceipts = stored.pending
            receiptStorageLoaded = true
            if defaults.object(forKey: preferenceKey) != nil || defaults.object(forKey: preferenceKey + ".pending") != nil {
                if !saveReceipts() { receiptStorageLoaded = false; return }
                defaults.removeObject(forKey: preferenceKey)
                defaults.removeObject(forKey: preferenceKey + ".pending")
            }
            receiptError = nil
            hasReceiptStorageError = false
        } catch {
            receiptStorageLoaded = false
            hasReceiptStorageError = true
            receiptError = String(localized: "Notification changes couldn't be saved on this Mac. Try again.")
        }
    }

    private func sendPendingReceipt(_ pending: PendingInboxReceipt) async -> Bool {
        guard receiptStorageLoaded else { return false }
        guard pendingReceipts.contains(where: { $0.id == pending.id }), !updatingIds.contains(pending.notificationId) else { return true }
        let requestGeneration = generation
        let authority = context.authorityGeneration
        updatingIds.insert(pending.notificationId)
        defer { if generation == requestGeneration { updatingIds.remove(pending.notificationId) } }
        do {
            try await updateReceipt(pending.notificationId, .init(version: pending.version, action: pending.action))
            try context.ensureAuthority(authority)
            guard generation == requestGeneration else { return false }
            receiptGeneration = UUID()
            if let index = remoteItems.firstIndex(where: {
                $0.id == pending.notificationId && $0.serverVersion == pending.version
            }) { Self.apply(pending.action, to: &remoteItems[index]) }
        } catch {
            guard generation == requestGeneration, context.authorityGeneration == authority else { return false }
            let failure = ClientFailure(error)
            if failure.canRetryReceipt || failure == .authentication || failure == .cancelled {
                // The persisted intent remains visible and survives relaunch until acknowledged.
                return true
            }
            pendingReceipts.removeAll { $0.id == pending.id }
            let saved = saveReceipts()
            publishItems()
            guard saved else { return false }
            receiptError = failure == .invalidInput
                ? String(localized: "Couldn't update this notification. Refresh Inbox and try again.")
                : error.actionMessage
            return false
        }
        let prior = pendingReceipts
        pendingReceipts.removeAll { $0.id == pending.id }
        guard saveReceipts() else { pendingReceipts = prior; publishItems(); return false }
        receiptError = nil
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
        _ = saveReceipts()
    }

    @discardableResult
    private func saveReceipts() -> Bool {
        guard let preferenceKey, receiptStorageLoaded else { return false }
        do {
            let file = receiptFile(for: preferenceKey)
            try FileManager.default.createDirectory(at: receiptDirectory, withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700])
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: receiptDirectory.path)
            let data = try JSONEncoder().encode(InboxReceiptState(version: 1, local: localReceipts, pending: pendingReceipts))
            try data.write(to: file, options: .atomic)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
            if hasReceiptStorageError { receiptError = nil }
            hasReceiptStorageError = false
            return true
        } catch {
            hasReceiptStorageError = true
            receiptError = String(localized: "Notification changes couldn't be saved on this Mac. Try again.")
            return false
        }
    }

    private func retainProjects(_ accessible: Set<String>) {
        remoteItems.removeAll { $0.type != .accessChanges && $0.projectId.map { !accessible.contains($0) } == true }
        localItems.removeAll { $0.projectId.map { !accessible.contains($0) } == true }
        // Published emits before context.projects changes; use the incoming membership set.
        publishItems(accessibleProjects: accessible)
    }

    private func publishItems(accessibleProjects: Set<String>? = nil) {
        let accessible = accessibleProjects ?? Set(context.projects.map(\.id))
        let displayedRemote = remoteItems.map { item in
            var item = item
            for pending in pendingReceipts where pending.notificationId == item.id && pending.version == item.serverVersion {
                Self.apply(pending.action, to: &item)
            }
            return item
        }
        items = (displayedRemote + localItems).map { item in
            var item = item
            if case .project(let projectId) = item.destination,
               !accessible.contains(projectId) {
                item.destination = nil
            }
            return item
        }.sorted {
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

    /// Remove only the selected local association; drafts and repository files remain.
    func removeUnavailableBinding(_ binding: DaemonProjectBinding) async throws {
        let authority = context.authorityGeneration
        let adapters = try await context.daemon.projectAgentAdapters(binding.projectId)
        try context.ensureAuthority(authority)
        for adapter in adapters where adapter.workspaceRoot == binding.workspaceRoot {
            _ = try await context.daemon.removeProjectAgentAdapter(.init(
                workspaceRoot: binding.workspaceRoot, adapter: adapter.adapter, expectedRevision: adapter.revision))
            try context.ensureAuthority(authority)
        }
        _ = try await context.daemon.removeProjectBinding(.init(
            workspaceRoot: binding.workspaceRoot, expectedRevision: binding.revision))
        try context.ensureAuthority(authority)
        await refresh()
    }

    /// Export complete local operation history without requiring remote project access.
    func exportUnavailableDrafts(_ projectId: String, to destination: URL) async throws {
        let authority = context.authorityGeneration
        let daemon = context.daemon
        let summaries = try await WorkspaceLoader.listAllDraftSummaries { query in
            try await daemon.listDrafts(query)
        }
        try context.ensureAuthority(authority)
        var drafts: [DaemonDraftDetail] = []
        for summary in summaries where summary.projectId == projectId && [.open, .submitted].contains(summary.status) {
            drafts.append(try await context.daemon.draft(summary.draftId))
            try context.ensureAuthority(authority)
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(drafts).write(to: destination, options: .atomic)
    }

    static func localNotifications(_ sync: DaemonSyncStatus) -> [InboxItem] {
        var notices: [InboxItem] = sync.unavailableProjects.map { project in
            .init(id: "local:project:\(project.projectId)", type: .accessChanges,
                projectId: nil, projectName: project.name,
                title: String(localized: "Project sync paused"),
                message: String(localized: "This project was deleted or is no longer accessible. Drafts on this Mac are retained."),
                occurredAt: .distantPast, needsAction: true,
                revision: "unavailable:\(project.draftCount)", isRead: false, isArchived: false,
                destination: .manageLocalProjects)
        }
        if sync.failedOperationCount > 0 || [sync.draftSync.state, sync.commitSync.state].contains(where: { ["failed", "degraded"].contains($0) }) {
            notices.append(.init(id: "local:sync", type: .syncErrors, projectId: nil, projectName: String(localized: "This Mac"),
                title: String(localized: "Changes couldn't sync"),
                message: String(localized: "Some changes haven't reached the server. Retry when connected."),
                occurredAt: .distantPast, needsAction: true, revision: "sync-failed", isRead: false, isArchived: false,
                destination: .retrySync))
        }
        return notices
    }
}
