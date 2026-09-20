import SwiftUI

/// Per-conflict choices, shared by document reconciliation and Review details.
struct DraftConflictView: View {
    let candidate: DraftReconciliationCandidate
    @Binding var resolution: DraftResolution
    @State private var customPath = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if hasExistenceConflict {
                Text("One version deletes this file. Choose whether to keep it.")
                    .font(.callout).foregroundStyle(.secondary)
                HStack(alignment: .top, spacing: 12) {
                    fileChoice("Remote", state: candidate.currentState)
                    fileChoice("Draft", state: candidate.draftState)
                }
            }
            if hasPathConflict {
                HStack(alignment: .top, spacing: 12) {
                    choice("Remote", text: candidate.currentState.resource.path ?? "(No path)",
                           actionTitle: "Use Remote Path") {
                        resolution.choosePath(candidate.currentState.resource.path ?? "")
                    }
                    choice("Draft", text: candidate.draftState.resource.path ?? "(No path)",
                           actionTitle: "Use Draft Path") {
                        resolution.choosePath(candidate.draftState.resource.path ?? "")
                    }
                }
            }
            if hasPathConflict {
                HStack {
                    TextField("Custom path", text: $customPath)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit { if !customPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { resolution.choosePath(customPath) } }
                    Button { resolution.choosePath(customPath) } label: { Image(systemName: "checkmark") }
                        .disabled(customPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .help("Use custom path")
                        .accessibilityLabel("Use Custom Path")
                }
            }
            ForEach(resolution.sections) { section in
                HStack(alignment: .top, spacing: 12) {
                    choice("Remote", text: section.shared, original: section.base,
                           actionTitle: "Use Remote Change") {
                        resolution.chooseContent(section.shared, in: section)
                    }
                    choice("Draft", text: section.proposed, original: section.base,
                           actionTitle: "Use Draft Change") {
                        resolution.chooseContent(section.proposed, in: section)
                    }
                }
            }
            if resolution.unresolvedFields.contains("content") {
                HStack(alignment: .top, spacing: 12) {
                    fileChoice("Remote", state: candidate.currentState)
                    fileChoice("Draft", state: candidate.draftState)
                }
            }
        }
        .accessibilityIdentifier("draft-conflict-choices")
    }

    private func choice(_ title: String, text: String, original: String? = nil, actionTitle: String,
            action: @escaping () -> Void) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(title).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
                    .help(original == nil ? title : "Changes from the common original to \(title)")
                Spacer()
                Button(actionTitle, action: action).controlSize(.small)
            }.padding(12)
            Divider()
            if let original, original != text {
                UnifiedDiffView(presentation: UnifiedDiffPresentation(
                    model: SplitDiffModel.make(original: original, modified: text)
                ))
            } else {
                Text(text.isEmpty ? "(Removed)" : text)
                    .font(.system(.body, design: .monospaced)).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
    }

    private func fileChoice(_ title: String, state: ReconciliationResourceState) -> some View {
        choice(title, text: text(in: state), original: text(in: candidate.baseState),
               actionTitle: state.exists ? "Keep \(title) File" : "Keep File Deleted") {
            resolution.chooseFile(state)
        }
    }

    private var hasExistenceConflict: Bool { candidate.conflicts.contains { $0.field == "exists" } }
    private var hasPathConflict: Bool { candidate.conflicts.contains { $0.field == "path" || $0.field == "path_occupied" } }

    private func text(in state: ReconciliationResourceState) -> String {
        state.exists ? state.content?.primaryText ?? "" : ""
    }
}
