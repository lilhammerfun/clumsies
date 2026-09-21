import Combine
import Foundation

@MainActor
final class ReviewRequestModel: ObservableObject {
    private let loadCandidates: () async throws -> [DraftReconciliationCandidate]
    let onSubmit: (String, String, [ReviewDraftReconciliation]) async throws -> Void

    @Published var title: String
    @Published var description = ""
    @Published private(set) var isSubmitting = false
    @Published var errorMessage: String?
    @Published private(set) var reconciliationCandidates = [DraftReconciliationCandidate]()
    @Published var resolvedStatesByCandidateId = [String: ReconciliationResourceState]()
    @Published var conflictIndex = 0

    init(initialTitle: String,
         loadCandidates: @escaping () async throws -> [DraftReconciliationCandidate],
         onSubmit: @escaping (String, String, [ReviewDraftReconciliation]) async throws -> Void) {
        title = initialTitle
        self.loadCandidates = loadCandidates
        self.onSubmit = onSubmit
    }

    var normalizedTitle: String {
        title.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var normalizedDescription: String {
        description.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var activeConflictCandidate: DraftReconciliationCandidate? {
        let conflicts = reconciliationCandidates.filter { $0.status == .conflicts }
        guard conflictIndex < conflicts.count else { return nil }
        return conflicts[conflictIndex]
    }

    func resetReconciliation() {
        reconciliationCandidates = []
        resolvedStatesByCandidateId = [:]
        conflictIndex = 0
    }

    func submit() async -> Bool {
        guard !isSubmitting, !normalizedTitle.isEmpty else { return false }
        isSubmitting = true
        errorMessage = nil
        defer { isSubmitting = false }
        do {
            try await onSubmit(normalizedTitle, normalizedDescription, [])
            return true
        } catch ReviewRequestError.reconciliationRequired {
            do {
                let candidates = try await loadCandidates()
                try Task.checkCancellation()
                if candidates.isEmpty {
                    try await onSubmit(normalizedTitle, normalizedDescription, [])
                    return true
                }
                reconciliationCandidates = candidates
                resolvedStatesByCandidateId = [:]
                conflictIndex = 0
            } catch where error.isUserCancellation {
            } catch {
                errorMessage = error.actionMessage
            }
        } catch where error.isUserCancellation {
        } catch {
            errorMessage = error.actionMessage
        }
        return false
    }

    func submitBatch() async -> Bool {
        guard !isSubmitting else { return false }
        isSubmitting = true
        errorMessage = nil
        defer { isSubmitting = false }
        let reconciliations = reconciliationCandidates.map { candidate in
            ReviewDraftReconciliation(candidate: candidate, resolvedState: resolvedStatesByCandidateId[candidate.candidateId])
        }
        do {
            try await onSubmit(normalizedTitle, normalizedDescription, reconciliations)
            return true
        } catch where error.isUserCancellation {
        } catch {
            errorMessage = error.actionMessage
        }
        return false
    }
}
