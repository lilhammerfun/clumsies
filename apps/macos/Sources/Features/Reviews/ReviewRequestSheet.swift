import AppKit
import SwiftUI

struct ReviewRequestSheet: View {
    @Environment(\.dismiss) private var dismiss

    let loadCandidates: () async throws -> [DraftReconciliationCandidate]
    let onSubmit: (String, String, [ReviewDraftReconciliation]) async throws -> Void

    @State private var title: String
    @State private var description = ""
    @State private var isSubmitting = false
    @State private var errorMessage: String?
    @State private var reconciliationCandidates = [DraftReconciliationCandidate]()
    @State private var resolvedStatesByCandidateId = [String: ReconciliationResourceState]()
    @State private var conflictIndex = 0

    init(
        initialTitle: String,
        loadCandidates: @escaping () async throws -> [DraftReconciliationCandidate],
        onSubmit: @escaping (String, String, [ReviewDraftReconciliation]) async throws -> Void
    ) {
        _title = State(initialValue: initialTitle)
        self.loadCandidates = loadCandidates
        self.onSubmit = onSubmit
    }

    var body: some View {
        Group {
            if reconciliationCandidates.count == 1,
               let candidate = reconciliationCandidates.first {
                DraftReconciliationView(
                    candidate: candidate,
                    onCancel: resetReconciliation,
                    onApplied: { dismiss() }
                ) { resolvedState in
                    try await onSubmit(
                        normalizedTitle,
                        normalizedDescription,
                        [.init(candidate: candidate, resolvedState: resolvedState)]
                    )
                }
                .frame(minWidth: 780, idealWidth: 980, minHeight: 560, idealHeight: 680)
            } else if let candidate = activeConflictCandidate {
                DraftReconciliationView(
                    candidate: candidate,
                    onCancel: resetReconciliation,
                    onApplied: { conflictIndex += 1 }
                ) { resolvedState in
                    if let resolvedState {
                        resolvedStatesByCandidateId[candidate.candidateId] = resolvedState
                    }
                }
                .id(candidate.candidateId)
                .frame(minWidth: 780, idealWidth: 980, minHeight: 560, idealHeight: 680)
            } else if !reconciliationCandidates.isEmpty {
                batchConfirmation
            } else {
                requestForm
            }
        }
        .interactiveDismissDisabled(isSubmitting)
        .alert(
            "Could Not Request Review",
            isPresented: Binding(
                get: { errorMessage != nil },
                set: { if !$0 { errorMessage = nil } }
            )
        ) {
            Button("OK") { errorMessage = nil }
        } message: {
            Text(errorMessage ?? "")
                .textSelection(.enabled)
        }
    }

    private var requestForm: some View {
        VStack(spacing: 0) {
            Form {
                Section("Review") {
                    TextField("Title", text: $title)
                    TextField("Description", text: $description, axis: .vertical)
                        .lineLimit(4...8)
                }
            }
            .formStyle(.grouped)

            Divider()

            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button {
                    submit()
                } label: {
                    if isSubmitting {
                        ProgressView()
                            .controlSize(.small)
                    } else {
                        Text("Request")
                    }
                }
                .disabled(isSubmitting || normalizedTitle.isEmpty)
                .keyboardShortcut(.defaultAction)
            }
            .padding(12)
        }
        .frame(width: 480, height: 270)
    }

    private var batchConfirmation: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Update Drafts and Request Review")
                    .font(.title2.weight(.semibold))
                Text("The latest shared changes will be applied to these drafts in one transaction.")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(20)

            Divider()

            List(reconciliationCandidates) { candidate in
                HStack(spacing: 10) {
                    Image(systemName: candidate.status == .conflicts
                        ? "checkmark.circle.fill"
                        : "arrow.trianglehead.merge")
                        .foregroundStyle(candidate.status == .conflicts ? .green : .secondary)
                    Text(candidatePath(candidate))
                        .font(.body.monospaced())
                    Spacer()
                    Text(candidate.status == .conflicts ? "Resolved" : "Clean")
                        .foregroundStyle(.secondary)
                }
            }

            Divider()

            HStack {
                Button("Back") { resetReconciliation() }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button {
                    submitBatch()
                } label: {
                    if isSubmitting {
                        ProgressView().controlSize(.small)
                    } else {
                        Text("Update and Request Review")
                    }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(isSubmitting || reconciliationCandidates.contains { !$0.valid })
            }
            .padding(12)
        }
        .frame(width: 620, height: 460)
    }

    private var normalizedTitle: String {
        title.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var normalizedDescription: String {
        description.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var activeConflictCandidate: DraftReconciliationCandidate? {
        let conflicts = reconciliationCandidates.filter { $0.status == .conflicts }
        guard conflictIndex < conflicts.count else { return nil }
        return conflicts[conflictIndex]
    }

    private func candidatePath(_ candidate: DraftReconciliationCandidate) -> String {
        candidate.proposedState?.resource.path
            ?? candidate.draftState.resource.path
            ?? candidate.currentState.resource.path
            ?? candidate.draftId
    }

    private func resetReconciliation() {
        reconciliationCandidates = []
        resolvedStatesByCandidateId = [:]
        conflictIndex = 0
    }

    private func submit() {
        guard !normalizedTitle.isEmpty else { return }
        isSubmitting = true
        Task {
            do {
                try await onSubmit(normalizedTitle, normalizedDescription, [])
                dismiss()
            } catch ReviewRequestError.reconciliationRequired {
                do {
                    let candidates = try await loadCandidates()
                    if candidates.isEmpty {
                        try await onSubmit(normalizedTitle, normalizedDescription, [])
                        dismiss()
                    } else {
                        reconciliationCandidates = candidates
                        resolvedStatesByCandidateId = [:]
                        conflictIndex = 0
                    }
                    isSubmitting = false
                } catch {
                    errorMessage = error.localizedDescription
                    isSubmitting = false
                }
            } catch {
                errorMessage = error.localizedDescription
                isSubmitting = false
            }
        }
    }

    private func submitBatch() {
        isSubmitting = true
        let reconciliations = reconciliationCandidates.map { candidate in
            ReviewDraftReconciliation(
                candidate: candidate,
                resolvedState: resolvedStatesByCandidateId[candidate.candidateId]
            )
        }
        Task {
            do {
                try await onSubmit(normalizedTitle, normalizedDescription, reconciliations)
                dismiss()
            } catch {
                errorMessage = error.localizedDescription
                isSubmitting = false
            }
        }
    }
}
