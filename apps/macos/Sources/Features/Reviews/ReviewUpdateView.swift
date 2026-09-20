import SwiftUI

/// Reconciliation stays in the selected file's detail, using the Review's shared choices.
struct ReviewUpdateView<Content: View>: View {
    @ObservedObject var model: ReviewUpdateModel
    let draftId: String
    @ViewBuilder let currentFile: () -> Content
    @State private var confirmsRestart = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if model.isLoading {
                ProgressView("Checking the latest remote version…")
            } else if let candidate = model.candidates.first(where: { $0.draftId == draftId }),
                      let resolution = model.resolutions[candidate.candidateId] {
                if resolution.canSave {
                    fileDiff(from: candidate.currentState, to: resolution.state)
                        .overlay(alignment: .topTrailing) {
                            if resolution.hasEdits {
                                Menu {
                                    Button("Reset File Choices") { model.resetResolution(for: candidate) }
                                } label: {
                                    Image(systemName: "ellipsis")
                                }
                                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize().padding(6)
                                .help("File Actions")
                                .accessibilityLabel("File Actions")
                            }
                        }
                } else {
                    DraftConflictView(candidate: candidate, resolution: Binding(
                        get: { model.resolutions[candidate.candidateId] ?? resolution },
                        set: { model.setResolution($0, for: candidate.candidateId) }
                    ))
                    .disabled(!model.canResolveConflicts)
                }
            } else if model.plan != nil {
                currentFile()
            }
            if let error = model.errorMessage {
                Text(error).foregroundStyle(.red).textSelection(.enabled)
                Button("Check Latest Again") {
                    if model.hasEdits { confirmsRestart = true }
                    else { Task { await model.load(restart: true) } }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .disabled(model.isApplying)
        .confirmationDialog("Check again and replace these resolution edits?", isPresented: $confirmsRestart) {
            Button("Discard Edits and Check Again", role: .destructive) {
                Task { await model.load(restart: true) }
            }
            Button("Keep Editing", role: .cancel) {}
        }
    }

    @ViewBuilder
    private func fileDiff(from remote: ReconciliationResourceState, to draft: ReconciliationResourceState) -> some View {
        if !draft.exists {
            Label("This file will be deleted.", systemImage: "trash")
                .font(.callout).foregroundStyle(.secondary)
        } else if remote.resource.path != draft.resource.path {
            Text("\(remote.resource.path ?? "/dev/null") → \(draft.resource.path ?? "/dev/null")")
                .font(.caption.monospaced()).textSelection(.enabled)
        }
        UnifiedDiffView(presentation: UnifiedDiffPresentation(model: .make(
            original: remote.exists ? remote.content?.primaryText ?? "" : "",
            modified: draft.exists ? draft.content?.primaryText ?? "" : ""
        )))
    }
}

struct ReviewUpdateMenuItem: View {
    @ObservedObject var model: ReviewUpdateModel
    let onApplied: (ReviewDetail) -> Void

    var body: some View {
        Button(model.isApplying ? "Saving Conflict Resolutions…" : "Save Conflict Resolutions") {
            Task { if let detail = await model.submit() { onApplied(detail) } }
        }
        .disabled(!model.canApply)
        .help(model.unresolvedCount > 0
            ? "Save Conflict Resolutions — choose Remote or Draft for each conflict before saving"
            : "Save Conflict Resolutions — save your conflict choices without approving or publishing the Review")
        .accessibilityLabel("Save Conflict Resolutions")
        .accessibilityIdentifier("review-menu-update")
    }
}
