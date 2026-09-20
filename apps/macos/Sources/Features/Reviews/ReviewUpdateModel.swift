import Combine
import Foundation

@MainActor
final class ReviewUpdateModel: ObservableObject {
    let review: ReviewRecord
    private let prepare: () async throws -> ReviewUpdatePlan
    private let apply: (ReviewUpdatePlan, CreateReviewUpdateRequest) async throws -> ReviewDetail
    private var generation = UUID()

    @Published private(set) var plan: ReviewUpdatePlan?
    @Published private(set) var isLoading = false
    @Published private(set) var isApplying = false
    @Published private(set) var errorMessage: String?
    @Published var selectedCandidateId: String?
    @Published private(set) var resolutions: [String: DraftResolution] = [:]
    @Published private(set) var hasEdits = false

    init(review: ReviewRecord,
         prepare: @escaping () async throws -> ReviewUpdatePlan,
         apply: @escaping (ReviewUpdatePlan, CreateReviewUpdateRequest) async throws -> ReviewDetail) {
        self.review = review
        self.prepare = prepare
        self.apply = apply
    }

    var candidates: [DraftReconciliationCandidate] { plan?.candidates ?? [] }
    var selectedCandidate: DraftReconciliationCandidate? {
        candidates.first { $0.candidateId == selectedCandidateId }
    }
    var unresolvedCount: Int {
        candidates.filter { $0.status == .conflicts && resolutions[$0.candidateId]?.canSave != true }.count
    }
    var canApply: Bool {
        plan != nil && !candidates.isEmpty && !isLoading && !isApplying
            && candidates.allSatisfy(\.valid) && unresolvedCount == 0
    }

    func load(restart: Bool = false) async {
        guard (plan == nil || restart), !isLoading, !isApplying else { return }
        let request = generation
        isLoading = true
        errorMessage = nil
        defer { if generation == request { isLoading = false } }
        do {
            let result = try await prepare()
            guard generation == request, !Task.isCancelled else { return }
            plan = result
            resolutions = [:]
            hasEdits = false
            for candidate in result.candidates {
                resolutions[candidate.candidateId] = DraftResolution(candidate: candidate)
            }
            selectedCandidateId = result.candidates.first(where: { $0.status == .conflicts })?.candidateId
                ?? result.candidates.first?.candidateId
        } catch {
            guard generation == request, !Task.isCancelled else { return }
            errorMessage = error.localizedDescription
        }
    }

    func setResolution(_ state: DraftResolution, for candidateId: String) {
        guard !isApplying else { return }
        resolutions[candidateId] = state
        hasEdits = true
    }

    func submit() async -> ReviewDetail? {
        guard canApply, let plan else { return nil }
        let requestGeneration = generation
        isApplying = true
        errorMessage = nil
        defer { if generation == requestGeneration { isApplying = false } }
        let drafts = plan.detail.drafts ?? [.init(draft: plan.detail.draft, operations: plan.detail.operations)]
        let request = CreateReviewUpdateRequest(
            expectedReviewVersion: plan.detail.review.version,
            drafts: drafts.map { item in
                let candidate = candidates.first { $0.draftId == item.draft.draftId }
                return ReviewDraftRequest(
                    draftId: item.draft.draftId, expectedDraftVersion: item.draft.version,
                    candidateId: candidate?.candidateId,
                    resolvedState: candidate.flatMap { $0.status == .conflicts ? resolutions[$0.candidateId]?.state : nil }
                )
            }
        )
        do {
            let result = try await apply(plan, request)
            guard generation == requestGeneration, !Task.isCancelled else { return nil }
            return result
        } catch {
            guard generation == requestGeneration, !Task.isCancelled else { return nil }
            errorMessage = error.localizedDescription
            return nil
        }
    }

    func invalidate() {
        generation = UUID()
        plan = nil
        resolutions = [:]
        hasEdits = false
        isLoading = false
        isApplying = false
        errorMessage = nil
    }
}
