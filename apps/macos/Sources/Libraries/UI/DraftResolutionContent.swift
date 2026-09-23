import SwiftUI

/// The same per-change choices and resulting diff in Review details and draft sheets.
struct DraftResolutionContent: View {
    let candidate: DraftReconciliationCandidate
    @Binding var resolution: DraftResolution

    var body: some View {
        if resolution.canSave {
            resultDiff
                .overlay(alignment: .topTrailing) {
                    if resolution.hasEdits {
                        Menu {
                            Button("Reset File Choices") { resolution = DraftResolution(candidate: candidate) }
                        } label: {
                            Image(systemName: "ellipsis")
                        }
                        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize().padding(6)
                        .help("File Actions")
                        .accessibilityLabel("File Actions")
                    }
                }
        } else {
            DraftConflictView(candidate: candidate, resolution: $resolution)
        }
    }

    @ViewBuilder
    private var resultDiff: some View {
        let remote = candidate.currentState
        let draft = resolution.state
        if !draft.exists {
            Label("This file will be deleted.", systemImage: "trash")
                .font(.callout).foregroundStyle(.secondary)
        } else if remote.resource.path != draft.resource.path {
            Text("\(remote.resource.path ?? "/dev/null") → \(draft.resource.path ?? "/dev/null")")
                .font(.caption.monospaced()).textSelection(.enabled)
        }
        if remote == draft {
            ContentUnavailableView("No Draft Changes", systemImage: "doc.text",
                description: Text("Saving brings this draft up to date without leaving changes to publish."))
        } else {
            UnifiedDiffView(presentation: UnifiedDiffPresentation(model: .make(
                original: remote.exists ? remote.content?.primaryText ?? "" : "",
                modified: draft.exists ? draft.content?.primaryText ?? "" : ""
            )))
        }
    }
}
