import Combine
import Foundation

@MainActor
final class ReviewRequestModel: ObservableObject {
    private let loadCandidates: () async throws -> [DraftReconciliationCandidate]
    let onSubmit: (String, String, [ReviewDraftReconciliation], [OrgContributionEntry]) async throws -> Void

    let drafts: [LocalDraft]
    @Published var contributesToOrg = false
    @Published var contributionDraftIds = Set<String>()
    @Published var contributionTargets = [String: String]()
    @Published var contributionPaths = [String: String]()

    var contributionEntries: [OrgContributionEntry] {
        guard contributesToOrg else { return [] }
        return contributableDrafts.filter { contributionDraftIds.contains($0.id) }.map { draft in
            let target = contributionTargets[draft.id] ?? ""
            return .init(draftId: draft.id, targetId: target.isEmpty ? nil : target,
                         path: target.isEmpty ? contributionPaths[draft.id] ?? draft.document.path : nil)
        }
    }

    var contributableDrafts: [LocalDraft] { drafts.filter { $0.scope == .project && !$0.isDeletion } }

    var canContribute: Bool {
        !contributableDrafts.isEmpty && drafts.allSatisfy { $0.scope == .project }
    }

    @Published var title: String
    @Published var description = ""
    @Published private(set) var isSubmitting = false
    @Published var errorMessage: String?
    @Published private(set) var reconciliationCandidates = [DraftReconciliationCandidate]()
    @Published var resolvedStatesByCandidateId = [String: ReconciliationResourceState]()
    @Published var conflictIndex = 0

    init(initialTitle: String, drafts: [LocalDraft] = [],
         loadCandidates: @escaping () async throws -> [DraftReconciliationCandidate],
         onSubmit: @escaping (String, String, [ReviewDraftReconciliation], [OrgContributionEntry]) async throws -> Void) {
        title = initialTitle
        self.drafts = drafts
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
        if contributesToOrg && contributionEntries.isEmpty {
            errorMessage = String(localized: "Select at least one document to contribute.")
            return false
        }
        isSubmitting = true
        errorMessage = nil
        defer { isSubmitting = false }
        do {
            try await onSubmit(normalizedTitle, normalizedDescription, [], contributionEntries)
            return true
        } catch ReviewRequestError.reconciliationRequired {
            do {
                let candidates = try await loadCandidates()
                try Task.checkCancellation()
                if candidates.isEmpty {
                    try await onSubmit(normalizedTitle, normalizedDescription, [], contributionEntries)
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
            try await onSubmit(normalizedTitle, normalizedDescription, reconciliations, contributionEntries)
            return true
        } catch where error.isUserCancellation {
        } catch {
            errorMessage = error.actionMessage
        }
        return false
    }
}
