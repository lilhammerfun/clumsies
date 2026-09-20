import Combine
import Foundation

private struct SyncRetryTaskHandle {
    let id: UUID
    let task: Task<SyncRetryOutcome, Never>
}

@MainActor
final class DaemonSyncService: ObservableObject {
    private let context: WorkspaceContext
    private let feedback: WorkspaceFeedback
    var onRetryCompleted: (() async -> Void)?

    init(context: WorkspaceContext, feedback: WorkspaceFeedback) {
        self.context = context
        self.feedback = feedback
    }

    @Published var runtime: RuntimeState?
    @Published var syncStatusAvailable = true
    @Published var retryingSyncKeys: Set<SyncRetryKey> = []

    var postReadyRetrySyncTask: Task<Void, Never>?

    var isRefreshingSyncStatus = false

    var isRefreshingSynchronizedWorkspaceData = false

    private var syncRetryTasks: [SyncRetryKey: SyncRetryTaskHandle] = [:]

    var isRetryingSync: Bool {
        retryingSyncKeys.contains { $0.projectId == self.context.activeProjectId }
    }

    var syncRetryErrorMessage: String? {
        let messages = feedback.syncRetryErrors
            .filter { $0.key.projectId == self.context.activeProjectId }
            .sorted { $0.key.channel < $1.key.channel }
            .map(\.value)
        return messages.isEmpty ? nil : messages.joined(separator: "\n")
    }

    func isRetryingSync(channel: String, projectId: String?) -> Bool {
        retryingSyncKeys.contains(
            SyncRetryKey(channel: channel, projectId: projectId)
        )
    }

    func cancelSyncRetries() {
        if let key = feedback.presentedSyncRetryErrorKey,
           feedback.errorMessage == self.feedback.syncRetryErrors[key] {
            feedback.errorMessage = nil
        }
        syncRetryTasks.values.forEach { $0.task.cancel() }
        syncRetryTasks.removeAll()
        retryingSyncKeys.removeAll()
        feedback.syncRetryErrors.removeAll()
        feedback.presentedSyncRetryErrorKey = nil
    }

    func clearSyncRetryErrors(channel: String, projectId: String?) {
        let errorKeysToClear = channel == "all"
            ? feedback.syncRetryErrors.keys.filter { $0.projectId == projectId }
            : [SyncRetryKey(channel: channel, projectId: projectId)]
        let presentedError = feedback.presentedSyncRetryErrorKey.flatMap { self.feedback.syncRetryErrors[$0] }
        if let presentedSyncRetryErrorKey = feedback.presentedSyncRetryErrorKey,
           errorKeysToClear.contains(presentedSyncRetryErrorKey),
           feedback.errorMessage == presentedError {
            feedback.errorMessage = nil
            feedback.presentedSyncRetryErrorKey = nil
        }
        for errorKey in errorKeysToClear {
            feedback.syncRetryErrors[errorKey] = nil
        }
    }

    @discardableResult
    func retrySync(
        channel: String = "all",
        projectId: String? = nil,
        allProjects: Bool = false
    ) async -> SyncRetryOutcome {
        let projectId = allProjects ? nil : projectId ?? context.activeProjectId
        let key = SyncRetryKey(channel: channel, projectId: projectId)
        if let inFlight = syncRetryTasks[key] {
            return await inFlight.task.value
        }

        clearSyncRetryErrors(channel: channel, projectId: projectId)
        let predecessors = syncRetryTasks.compactMap { existingKey, handle in
            existingKey.projectId == projectId ? handle.task : nil
        }

        let taskId = UUID()
        let task = Task { @MainActor in
            do {
                for predecessor in predecessors {
                    _ = await predecessor.value
                    try Task.checkCancellation()
                }
                self.clearSyncRetryErrors(channel: channel, projectId: projectId)
                try Task.checkCancellation()
                _ = try await self.context.daemon.retrySync(channel: channel, projectId: projectId)
                try Task.checkCancellation()
                if channel == "all" {
                    self.clearSyncRetryErrors(channel: channel, projectId: projectId)
                }
                if allProjects || self.context.activeProjectId == projectId {
                    await self.onRetryCompleted?()
                    try Task.checkCancellation()
                }
                return SyncRetryOutcome.completed
            } catch is CancellationError {
                return .cancelled
            } catch {
                guard !Task.isCancelled else { return .cancelled }
                let message = error.localizedDescription
                self.feedback.syncRetryErrors[key] = message
                if self.context.activeProjectId == projectId {
                    self.syncStatusAvailable = false
                    if self.feedback.errorMessage == nil {
                        self.feedback.errorMessage = message
                        self.feedback.presentedSyncRetryErrorKey = key
                    }
                }
                return .failed(message)
            }
        }
        syncRetryTasks[key] = .init(id: taskId, task: task)
        retryingSyncKeys.insert(key)
        let outcome = await task.value
        if syncRetryTasks[key]?.id == taskId {
            syncRetryTasks[key] = nil
            retryingSyncKeys.remove(key)
        }
        return outcome
    }

    func refreshSyncStatus() async {
        guard context.phase == .ready, !isRefreshingSyncStatus else { return }
        isRefreshingSyncStatus = true
        defer { isRefreshingSyncStatus = false }
        let generation = context.workspaceReloadGeneration
        let projectId = context.activeProjectId
        guard let runtime = runtime else { return }
        do {
            let sync = try await context.daemon.syncStatus(projectId: projectId)
            try Task.checkCancellation()
            guard context.workspaceReloadGeneration == generation,
                  context.activeProjectId == projectId,
                  context.phase == .ready else {
                return
            }
            let updatedRuntime = RuntimeState(
                health: runtime.health,
                sync: sync,
                serverDataSource: context.server.dataSource
            )
            if self.runtime != updatedRuntime {
                self.runtime = updatedRuntime
            }
            if !syncStatusAvailable {
                syncStatusAvailable = true
            }
        } catch is CancellationError {
            return
        } catch {
            guard context.workspaceReloadGeneration == generation,
                  context.activeProjectId == projectId else {
                return
            }
            if syncStatusAvailable {
                syncStatusAvailable = false
            }
        }
    }

    func startLoading(generation: UUID) {
        cancelLoading()
        postReadyRetrySyncTask = Task { @MainActor [weak self] in
            defer {
                if let self, self.context.workspaceReloadGeneration == generation {
                    self.postReadyRetrySyncTask = nil
                }
            }
            guard let self,
                  context.workspaceReloadGeneration == generation,
                  context.phase == .ready,
                  !Task.isCancelled else {
                return
            }
            _ = await retrySync(projectId: context.activeProjectId)
        }
    }

    func cancelLoading() {
        postReadyRetrySyncTask?.cancel()
        postReadyRetrySyncTask = nil
    }

    func resetAuthority() {
        cancelLoading()
        cancelSyncRetries()
        runtime = nil
        syncStatusAvailable = false
    }

    func applyRuntime(_ runtime: RuntimeState) {
        self.runtime = runtime
        syncStatusAvailable = false
    }
}

enum SyncRetryOutcome: Equatable, Sendable {
    case completed
    case failed(String)
    case cancelled
}

struct SyncRetryKey: Hashable, Sendable {
    let channel: String
    let projectId: String?
}
