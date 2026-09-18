import Combine
import Foundation

@MainActor
final class DraftReconciliationService: ObservableObject {
    private let catalog: MemoryCatalog
    private let context: WorkspaceContext
    private let edits: DraftStore
    private let refresh: DaemonSyncService
    private let sessions: DocumentSessions
    var onReconciled: (() async -> Void)?

    init(catalog: MemoryCatalog, context: WorkspaceContext, edits: DraftStore, refresh: DaemonSyncService, sessions: DocumentSessions) {
        self.catalog = catalog
        self.context = context
        self.edits = edits
        self.refresh = refresh
        self.sessions = sessions
    }

    nonisolated static func draftUploadBarrierDecision(
        serverDraftId: String?,
        pendingOperationCount: Int,
        failedOperationCount: Int,
        operationStates: [DaemonDraftSyncState],
        failureMessage: String?
    ) -> DraftUploadBarrierDecision {
        if failedOperationCount > 0 || operationStates.contains(.failed) {
            return .failed(failureMessage)
        }
        if pendingOperationCount == 0,
           serverDraftId != nil,
           operationStates.allSatisfy({ $0 == .synced }) {
            return .ready
        }
        return .wait
    }

    func synchronizedDraftForReconciliation(
        itemId: String,
        draft: LocalDraft
    ) async throws -> LocalDraft {
        let sessionKey = MemoryDocumentSessionKey(
            projectId: draft.projectId,
            itemId: itemId
        )
        if edits.pendingDocumentSaves[sessionKey] != nil {
            try await edits.flushDocumentSave(sessionKey)
        }

        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(15))
        var requestedRetry = false
        var requiresPostRetryCheck = false
        while clock.now < deadline || requiresPostRetryCheck {
            requiresPostRetryCheck = false
            try Task.checkCancellation()
            let detail = try await context.daemon.draft(draft.id)
            let failure = detail.operations.reversed().first {
                $0.syncStatus == .failed
            }?.lastError
            switch Self.draftUploadBarrierDecision(
                serverDraftId: detail.draft.serverDraftId,
                pendingOperationCount: detail.draft.pendingOperationCount,
                failedOperationCount: detail.draft.failedOperationCount,
                operationStates: detail.operations.map(\.syncStatus),
                failureMessage: failure
            ) {
            case .failed(let message):
                throw DocumentSyncError.draftUploadFailed(message)
            case .wait:
                if !requestedRetry {
                    let outcome = await refresh.retrySync(
                        channel: "drafts",
                        projectId: draft.projectId
                    )
                    if case .failed(let message) = outcome {
                        throw DocumentSyncError.draftUploadFailed(message)
                    }
                    requestedRetry = true
                    requiresPostRetryCheck = true
                    continue
                }
            case .ready:
                // A user edit may have been staged while the daemon was
                // uploading. Flush it and repeat the barrier before creating
                // a candidate with an authoritative serverVersion.
                if edits.pendingDocumentSaves[sessionKey] != nil {
                    try await edits.flushDocumentSave(sessionKey)
                    requestedRetry = false
                    requiresPostRetryCheck = true
                    continue
                }
                let mapped = WorkspaceLoader.mapDraft(detail, resources: catalog.resources)
                return mapped
            }
            try await Task.sleep(for: .milliseconds(150))
        }
        throw DocumentSyncError.draftUploadTimedOut
    }

    func reconciliationCandidate(for draft: LocalDraft) async throws -> DraftReconciliationCandidate {
        let itemId = draft.targetId ?? draft.id
        let key = MemoryDocumentSessionKey(projectId: draft.projectId, itemId: itemId)
        guard !context.isSwitchingMemoryContext,
              context.activeProjectId == draft.projectId,
              sessions.synchronizingDocumentSessions.insert(key).inserted else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        let generation = UUID()
        sessions.documentSynchronizationGenerations[key] = generation
        defer { self.sessions.endDocumentSynchronization(key, generation: generation) }

        let synchronized = try await synchronizedDraftForReconciliation(
            itemId: itemId,
            draft: draft
        )
        try Task.checkCancellation()
        guard sessions.isCurrentDocumentSynchronization(key, generation: generation) else {
            throw CancellationError()
        }
        let candidate = try await requestReconciliationCandidate(for: synchronized)
        try Task.checkCancellation()
        guard sessions.isCurrentDocumentSynchronization(key, generation: generation) else {
            throw CancellationError()
        }
        return candidate
    }

    func reconciliationCandidates(
        for drafts: [LocalDraft]
    ) async throws -> [DraftReconciliationCandidate] {
        guard !context.isSwitchingMemoryContext else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        let selectedDrafts = drafts.sorted {
            $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending
        }
        var candidates = [DraftReconciliationCandidate]()
        for draft in selectedDrafts {
            let synchronized = try await synchronizedDraftForReconciliation(
                itemId: draft.targetId ?? draft.id,
                draft: draft
            )
            if synchronized.freshness == .behind {
                candidates.append(try await requestReconciliationCandidate(for: synchronized))
            }
        }
        return candidates
    }

    func requestReconciliationCandidate(
        for synchronized: LocalDraft
    ) async throws -> DraftReconciliationCandidate {
        guard let serverId = synchronized.serverId else {
            throw ReviewRequestError.draftNotSynchronized
        }
        return try await context.server.send(
            method: "POST",
            path: "/api/v1/drafts/\(serverId)/reconciliation-candidates",
            body: CreateDraftReconciliationCandidateRequest(
                expectedDraftVersion: synchronized.serverVersion
            )
        )
    }

    func reconciliationCandidate(for detail: ReviewDetail) async throws -> DraftReconciliationCandidate {
        try await reconciliationCandidate(
            for: ReviewDraftDetail(draft: detail.draft, operations: detail.operations)
        )
    }

    func reconciliationCandidate(
        for detail: ReviewDraftDetail
    ) async throws -> DraftReconciliationCandidate {
        guard !context.isSwitchingMemoryContext else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        let activityId = UUID()
        sessions.standaloneReconciliationActivityIds.insert(activityId)
        defer { self.sessions.standaloneReconciliationActivityIds.remove(activityId) }
        return try await context.server.send(
            method: "POST",
            path: "/api/v1/drafts/\(detail.draft.draftId)/reconciliation-candidates",
            body: CreateDraftReconciliationCandidateRequest(
                expectedDraftVersion: detail.draft.version
            )
        )
    }

    func applyReconciliation(
        draftId: String,
        candidate: DraftReconciliationCandidate,
        resolvedState: ReconciliationResourceState?,
        projectId: String? = nil,
        documentItemId: String? = nil
    ) async throws {
        guard !context.isSwitchingMemoryContext else {
            throw DocumentSyncError.mutationWhileSynchronizing
        }
        var documentKey: MemoryDocumentSessionKey?
        var standaloneActivityId: UUID?
        if let documentItemId {
            guard let key = sessions.activeDocumentSessionKey(for: documentItemId),
                  sessions.synchronizingDocumentSessions.contains(key),
                  sessions.pendingDocumentReconciliationCandidatesBySession[key]?.candidateId
                    == candidate.candidateId,
                  sessions.applyingDocumentReconciliationSessions.insert(key).inserted else {
                throw DocumentSyncError.mutationWhileSynchronizing
            }
            documentKey = key
        } else {
            let activityId = UUID()
            sessions.standaloneReconciliationActivityIds.insert(activityId)
            standaloneActivityId = activityId
        }
        defer {
            if let documentKey {
                self.sessions.applyingDocumentReconciliationSessions.remove(documentKey)
            }
            if let standaloneActivityId {
                self.sessions.standaloneReconciliationActivityIds.remove(standaloneActivityId)
            }
        }
        guard let reconciliationProjectId = projectId ?? documentKey?.projectId else {
            throw ProjectMemorySelectionError.projectUnavailable
        }
        let _: DraftRebaseResult = try await context.server.send(
            method: "POST",
            path: "/api/v1/drafts/\(draftId)/rebases",
            headers: ["If-Match": Self.refETag(candidate.currentCommitId)],
            body: CreateDraftRebaseRequest(
                candidateId: candidate.candidateId,
                expectedDraftVersion: candidate.draftVersion,
                resolvedState: resolvedState
            )
        )
        _ = await refresh.retrySync(channel: "drafts", projectId: reconciliationProjectId)
        await onReconciled?()
    }

    static func refETag(_ commitId: String?) -> String {
        "\"\(commitId ?? "ref-none")\""
    }
}

enum DraftUploadBarrierDecision: Equatable, Sendable {
    case wait
    case ready
    case failed(String?)
}
