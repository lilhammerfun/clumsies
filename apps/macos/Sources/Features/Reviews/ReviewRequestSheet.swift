import AppKit
import SwiftUI

struct ReviewRequestSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var catalog: MemoryCatalog

    @StateObject private var model: ReviewRequestModel

    init(initialTitle: String, drafts: [LocalDraft] = [],
         loadCandidates: @escaping () async throws -> [DraftReconciliationCandidate],
         onSubmit: @escaping (String, String, [ReviewDraftReconciliation], [OrgContributionEntry]) async throws -> Void) {
        _model = StateObject(wrappedValue: ReviewRequestModel(
            initialTitle: initialTitle, drafts: drafts, loadCandidates: loadCandidates, onSubmit: onSubmit
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
                    onApplied: { if model.noticeMessage == nil { self.dismiss() } }
                ) { resolvedState in
                    do {
                        try await self.model.onSubmit(
                            self.model.normalizedTitle,
                            self.model.normalizedDescription,
                            [.init(candidate: candidate, resolvedState: resolvedState)],
                            self.model.contributionEntries
                        )
                    } catch ReviewRequestError.noChanges { model.reportNoChanges() }
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
    }

    private var requestForm: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    TextField("Title", text: self.$model.title)
                    TextField("Description", text: self.$model.description, axis: .vertical)
                        .lineLimit(4...8)
                } header: {
                    Text("Review")
                } footer: {
                    FormErrorMessage(message: model.errorMessage)
                    if let notice = model.noticeMessage { Text(notice).foregroundStyle(.secondary) }
                }
                if model.canContribute {
                    Section {
                        Toggle("After Project merge, propose an Organization contribution", isOn: $model.contributesToOrg)
                        if model.contributesToOrg {
                            ForEach(model.contributableDrafts) { draft in
                                Toggle(draft.document.path, isOn: Binding(
                                    get: { model.contributionDraftIds.contains(draft.id) },
                                    set: { selected in
                                        if selected { model.contributionDraftIds.insert(draft.id) }
                                        else { model.contributionDraftIds.remove(draft.id) }
                                    }
                                ))
                                if model.contributionDraftIds.contains(draft.id) {
                                    Picker("Organization destination", selection: Binding(
                                        get: { model.contributionTargets[draft.id] ?? "" },
                                        set: { model.contributionTargets[draft.id] = $0 }
                                    )) {
                                        Text("New Organization Memory").tag("")
                                        ForEach(catalog.resources.filter { $0.scope == .org }) { resource in
                                            Text(resource.document.path).tag(resource.id)
                                        }
                                    }
                                    if (model.contributionTargets[draft.id] ?? "").isEmpty {
                                        TextField("Organization path", text: Binding(
                                            get: { model.contributionPaths[draft.id] ?? draft.document.path },
                                            set: { model.contributionPaths[draft.id] = $0 }
                                        ))
                                    }
                                }
                            }
                            Text("The Organization proposal is reviewed separately. Project publication does not depend on its approval.")
                                .foregroundStyle(.secondary)
                        }
                    }
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
        .frame(width: 520, height: model.canContribute ? (model.contributesToOrg ? 560 : 340) : 270)
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

            if let error = model.errorMessage {
                FormErrorMessage(message: error).padding(.horizontal, 24).padding(.vertical, 12)
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
