import SwiftUI

struct ReviewUpdateView: View {
    @ObservedObject var model: ReviewUpdateModel
    let onCancel: () -> Void
    let onApplied: (ReviewDetail) -> Void
    @State private var confirmsDiscard = false
    @State private var confirmsRestart = false

    var body: some View {
        if #available(macOS 15.0, *) {
            content.presentationSizing(.fitted)
        } else {
            content
        }
    }

    private var content: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Update Review").font(.title2.weight(.semibold))
                Text(model.review.title).font(.headline)
                Text("This Review is based on an older remote version. Merge the remote changes with these drafts before approval.")
                    .foregroundStyle(.secondary)
                if model.plan != nil {
                    Text("\(model.candidates.count) files to update · \(model.unresolvedCount) unresolved")
                        .font(.callout)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(20)
            Divider()
            if model.isLoading {
                ProgressView("Checking all files against the latest remote version…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if model.plan == nil {
                ContentUnavailableView {
                    Label("Could Not Check Review", systemImage: "exclamationmark.triangle")
                } actions: {
                    Button("Try Again") { Task { await model.load() } }
                }
            } else if model.candidates.isEmpty {
                ContentUnavailableView("Already Up to Date", systemImage: "checkmark.circle",
                    description: Text("All active files use the latest remote version. Return to the Review to continue."))
            } else {
                HSplitView {
                    List(selection: $model.selectedCandidateId) {
                        ForEach(model.candidates) { candidate in
                            HStack {
                                Image(systemName: candidate.status == .clean
                                    || model.confirmed.contains(candidate.candidateId)
                                    ? "checkmark.circle" : "exclamationmark.triangle")
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(path(candidate)).lineLimit(2)
                                    Text(candidate.status == .clean ? "Merges automatically"
                                        : model.confirmed.contains(candidate.candidateId) ? "Resolved" : "Needs resolution")
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                            }
                            .tag(candidate.candidateId)
                            .help(path(candidate))
                        }
                    }
                    .frame(minWidth: 180, idealWidth: 250, maxWidth: 330)
                    .disabled(model.isApplying)

                    if let candidate = model.selectedCandidate {
                        VStack(alignment: .leading, spacing: 0) {
                            Text(path(candidate))
                                .font(.callout.monospaced()).textSelection(.enabled).padding(12)
                            Divider()
                            DraftReconciliationView(
                                candidate: candidate, usesContextualUpdateAction: true,
                                conflictMarkerLength: model.plan?.contentMerges[candidate.candidateId]?.markerLength,
                                initialResolvedState: model.resolutions[candidate.candidateId],
                                onResolvedStateChange: { model.setResolution($0, for: candidate.candidateId) },
                                onCancel: {}
                            ) { _ in }
                            .id(candidate.candidateId)
                            .disabled(model.isApplying)
                            if candidate.status == .conflicts {
                                Divider()
                                HStack {
                                    Text(model.confirmed.contains(candidate.candidateId)
                                        ? "This result is ready to apply with the other files."
                                        : "Resolve each highlighted section or edit the final result, then mark this file as resolved.")
                                        .font(.caption).foregroundStyle(.secondary)
                                    Spacer()
                                    Button("Mark Resolved") { model.confirm(candidate) }
                                        .disabled(!model.canConfirm(candidate) || model.confirmed.contains(candidate.candidateId))
                                }.padding(12)
                            }
                        }.frame(minWidth: 420, maxWidth: .infinity, maxHeight: .infinity)
                    }
                }
            }
            if let error = model.errorMessage {
                HStack {
                    Text(error).foregroundStyle(.red).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if model.plan != nil {
                        Button("Check Latest Again") {
                            if model.hasEdits { confirmsRestart = true }
                            else { Task { await model.load(restart: true) } }
                        }.disabled(model.isApplying || model.isLoading)
                    }
                }.padding(12)
            }
            Divider()
            HStack {
                Button(model.candidates.isEmpty && model.plan != nil ? "Back to Review" : "Cancel") {
                    if model.hasEdits { confirmsDiscard = true } else { onCancel() }
                }
                .keyboardShortcut(.cancelAction)
                .disabled(model.isApplying)
                Spacer()
                if model.isApplying {
                    ProgressView().controlSize(.small)
                    Text("Applying updates and refreshing the Review…").font(.caption)
                }
                Button("Apply All Updates") {
                    Task { if let result = await model.submit() { onApplied(result) } }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!model.canApply)
            }.padding(12)
        }
        .frame(minWidth: 1000, idealWidth: 1100, maxWidth: .infinity,
               minHeight: 720, idealHeight: 760, maxHeight: .infinity)
        .interactiveDismissDisabled(model.hasEdits || model.isApplying)
        .task { await model.load() }
        .confirmationDialog("Discard these resolution edits?", isPresented: $confirmsDiscard) {
            Button("Discard Edits", role: .destructive, action: onCancel)
            Button("Keep Editing", role: .cancel) {}
        }
        .confirmationDialog("Check again and replace these resolution edits?", isPresented: $confirmsRestart) {
            Button("Discard Edits and Check Again", role: .destructive) {
                Task { await model.load(restart: true) }
            }
            Button("Keep Editing", role: .cancel) {}
        }
    }

    private func path(_ candidate: DraftReconciliationCandidate) -> String {
        candidate.proposedState?.resource.path ?? candidate.draftState.resource.path
            ?? candidate.currentState.resource.path ?? "Untitled"
    }
}
