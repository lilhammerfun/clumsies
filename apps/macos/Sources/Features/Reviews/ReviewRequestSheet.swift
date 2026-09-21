import AppKit
import SwiftUI

struct ReviewRequestSheet: View {
    @Environment(\.dismiss) private var dismiss

    @StateObject private var model: ReviewRequestModel

    init(initialTitle: String,
         loadCandidates: @escaping () async throws -> [DraftReconciliationCandidate],
         onSubmit: @escaping (String, String, [ReviewDraftReconciliation]) async throws -> Void) {
        _model = StateObject(wrappedValue: ReviewRequestModel(
            initialTitle: initialTitle, loadCandidates: loadCandidates, onSubmit: onSubmit
        ))
    }

    var body: some View {
        Group {
            if self.model.reconciliationCandidates.count == 1,
               let candidate = model.reconciliationCandidates.first {
                DraftReconciliationView(
                    candidate: candidate,
                    updateButtonTitle: String(localized: "Save and Request Review"),
                    onCancel: self.model.resetReconciliation,
                    onApplied: { self.dismiss() }
                ) { resolvedState in
                    try await self.model.onSubmit(
                        self.model.normalizedTitle,
                        self.model.normalizedDescription,
                        [.init(candidate: candidate, resolvedState: resolvedState)]
                    )
                }
                .frame(minWidth: 780, idealWidth: 980, minHeight: 560, idealHeight: 680)
            } else if let candidate = model.activeConflictCandidate {
                DraftReconciliationView(
                    candidate: candidate,
                    updateButtonTitle: String(localized: "Use This Result"),
                    onCancel: self.model.resetReconciliation,
                    onApplied: { self.model.conflictIndex += 1 }
                ) { resolvedState in
                    if let resolvedState {
                        self.model.resolvedStatesByCandidateId[candidate.candidateId] = resolvedState
                    }
                }
                .id(candidate.candidateId)
                .frame(minWidth: 780, idealWidth: 980, minHeight: 560, idealHeight: 680)
            } else if !self.model.reconciliationCandidates.isEmpty {
                self.batchConfirmation
            } else {
                self.requestForm
            }
        }
        .interactiveDismissDisabled(model.isSubmitting)
        .alert(
            "Could Not Request Review",
            isPresented: Binding(
                get: { self.model.errorMessage != nil },
                set: { if !$0 { self.model.errorMessage = nil } }
            )
        ) {
            Button("OK") { self.model.errorMessage = nil }
        } message: {
            Text(self.model.errorMessage ?? "")
                .textSelection(.enabled)
        }
    }

    private var requestForm: some View {
        VStack(spacing: 0) {
            Form {
                Section("Review") {
                    TextField("Title", text: self.$model.title)
                    TextField("Description", text: self.$model.description, axis: .vertical)
                        .lineLimit(4...8)
                }
            }
            .formStyle(.grouped)
            .disabled(model.isSubmitting)

            SheetActionBar(
                confirmationTitle: Text("Request"), progressTitle: "Requesting review…",
                isWorking: model.isSubmitting, canConfirm: !model.normalizedTitle.isEmpty,
                cancel: { dismiss() }, confirm: {
                    Task { if await self.model.submit() { self.dismiss() } }
                }
            )
        }
        .frame(width: 480, height: 270)
    }

    private var batchConfirmation: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Update Drafts and Request Review")
                    .font(.title2.weight(.semibold))
                Text("All drafts will be updated to the latest remote version and submitted for review together. Nothing is published yet.")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(20)

            Divider()

            List(self.model.reconciliationCandidates) { candidate in
                HStack(spacing: 10) {
                    Image(systemName: candidate.status == .conflicts
                        ? "checkmark.circle.fill"
                        : "arrow.trianglehead.merge")
                        .foregroundStyle(candidate.status == .conflicts ? .green : .secondary)
                    Text(self.candidatePath(candidate))
                        .font(.body.monospaced())
                    Spacer()
                    Text(candidate.status == .conflicts ? "Resolved" : "Clean")
                        .foregroundStyle(.secondary)
                }
            }

            SheetActionBar(
                confirmationTitle: Text("Update and Request Review"), cancellationTitle: "Back",
                progressTitle: "Requesting review…", isWorking: model.isSubmitting,
                canConfirm: !model.reconciliationCandidates.contains { !$0.valid },
                cancel: model.resetReconciliation, confirm: {
                    Task { if await self.model.submitBatch() { self.dismiss() } }
                }
            )
        }
        .frame(width: 620, height: 460)
    }

    private func candidatePath(_ candidate: DraftReconciliationCandidate) -> String {
        candidate.proposedState?.resource.path
            ?? candidate.draftState.resource.path
            ?? candidate.currentState.resource.path
            ?? candidate.draftId
    }

}
