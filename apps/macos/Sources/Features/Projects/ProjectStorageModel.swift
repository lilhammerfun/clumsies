import Combine
import Foundation

@MainActor
final class ProjectStorageModel: ObservableObject {
    private struct Request {
        let id = UUID()
        let projectId: String
        let authority: UUID
    }

    private let context: WorkspaceContext
    private let fetchStorage: (String) async throws -> DaemonProjectStorage
    private var requestId = UUID()

    init(context: WorkspaceContext, fetchStorage: ((String) async throws -> DaemonProjectStorage)? = nil) {
        self.context = context
        let daemon = context.daemon
        self.fetchStorage = fetchStorage ?? { try await daemon.projectStorage($0) }
    }

    @Published private(set) var storage: DaemonProjectStorage?
    @Published private(set) var move: DaemonProjectStorageMove?
    @Published private(set) var isWorking = false
    @Published private(set) var errorMessage: String?

    func cancel() {
        requestId = UUID()
        isWorking = false
    }

    func loadStorage() async {
        cancel()
        storage = nil
        move = nil
        guard let projectId = context.activeProjectId else { return }
        let request = begin(projectId)
        defer { finish(request) }
        do {
            let loaded = try await fetchStorage(projectId)
            try check(request)
            storage = loaded
            if let moveId = loaded.activeMoveId {
                try await monitorMove(moveId, request: request)
            }
        } catch { fail(error, request: request) }
    }

    func chooseLocation(projectId: String, url: URL) async {
        guard let storage, storage.projectId == projectId, context.activeProjectId == projectId else { return }
        let request = begin(projectId)
        defer { finish(request) }
        do {
            let bookmark = try url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
            let created = try await context.daemon.replaceProjectStorage(.init(
                projectId: projectId, selectedRootPath: url.path,
                handoffBookmarkData: bookmark.base64EncodedString(), expectedLocationRevision: storage.locationRevision
            ))
            try check(request)
            move = created
            try await monitorMove(created.moveId, request: request)
        } catch { fail(error, request: request) }
    }

    func resetLocation() async {
        guard let storage, storage.projectId == context.activeProjectId else { return }
        let request = begin(storage.projectId)
        defer { finish(request) }
        do {
            let created = try await context.daemon.resetProjectStorage(.init(
                projectId: request.projectId, expectedLocationRevision: storage.locationRevision
            ))
            try check(request)
            move = created
            try await monitorMove(created.moveId, request: request)
        } catch { fail(error, request: request) }
    }

    func clearCache() async {
        guard let storage, storage.projectId == context.activeProjectId else { return }
        let request = begin(storage.projectId)
        defer { finish(request) }
        do {
            let cleared = try await context.daemon.clearProjectCache(.init(
                projectId: request.projectId, expectedLocationRevision: storage.locationRevision
            ))
            try check(request)
            self.storage = cleared
        } catch { fail(error, request: request) }
    }

    private func monitorMove(_ moveId: String, request: Request) async throws {
        while true {
            try check(request)
            let current = try await context.daemon.projectStorageMove(moveId)
            try check(request)
            move = current
            if current.state.isTerminal {
                errorMessage = current.errorMessage ?? (current.state == .failed ? String(localized: "The storage move failed.") : nil)
                let loaded = try await fetchStorage(request.projectId)
                try check(request)
                storage = loaded
                return
            }
            try await Task.sleep(for: .milliseconds(500))
        }
    }

    private func begin(_ projectId: String) -> Request {
        let request = Request(projectId: projectId, authority: context.authorityGeneration)
        requestId = request.id
        isWorking = true
        errorMessage = nil
        return request
    }

    private func check(_ request: Request) throws {
        try context.ensureAuthority(request.authority)
        guard requestId == request.id, context.activeProjectId == request.projectId else { throw CancellationError() }
    }

    private func finish(_ request: Request) {
        if requestId == request.id { isWorking = false }
    }

    private func fail(_ error: Error, request: Request) {
        guard requestId == request.id, context.activeProjectId == request.projectId,
              context.authorityGeneration == request.authority, !Task.isCancelled,
              !(error is CancellationError) else { return }
        errorMessage = error.localizedDescription
    }
}
