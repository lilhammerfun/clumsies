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
                    fileChoice(String(localized: "Remote"), state: candidate.currentState)
                    fileChoice(String(localized: "Draft"), state: candidate.draftState)
                }
            }
            if hasPathConflict {
                Text(candidate.conflicts.contains { $0.kind == "path_occupied" }
                    ? "Another file already uses this path. Choose a different path, even if the text is identical."
                    : "Both versions renamed this file. Choose the final path.")
                    .font(.callout).foregroundStyle(.secondary)
                HStack(alignment: .top, spacing: 12) {
                    choice(String(localized: "Remote"), text: candidate.currentState.resource.path ?? String(localized: "(No path)"),
                           actionTitle: String(localized: "Use Remote Path")) {
                        resolution.choosePath(candidate.currentState.resource.path ?? "")
                    }
                    choice(String(localized: "Draft"), text: candidate.draftState.resource.path ?? String(localized: "(No path)"),
                           actionTitle: String(localized: "Use Draft Path")) {
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
                    choice(String(localized: "Remote"), text: section.shared, original: section.base,
                           actionTitle: String(localized: "Use Remote Change")) {
                        resolution.chooseContent(section.shared, in: section)
                    }
                    choice(String(localized: "Draft"), text: section.proposed, original: section.base,
                           actionTitle: String(localized: "Use Draft Change")) {
                        resolution.chooseContent(section.proposed, in: section)
                    }
                }
            }
            if resolution.unresolvedFields.contains("content") {
                HStack(alignment: .top, spacing: 12) {
                    fileChoice(String(localized: "Remote"), state: candidate.currentState)
                    fileChoice(String(localized: "Draft"), state: candidate.draftState)
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
                    .help(original == nil ? title : String(localized: "Changes from the common original to \(title)"))
                Spacer()
                Button(actionTitle, action: action).controlSize(.small)
            }.padding(12)
            Divider()
            if let original, original != text {
                UnifiedDiffView(presentation: UnifiedDiffPresentation(
                    model: SplitDiffModel.make(original: original, modified: text)
                ))
            } else {
                Text(text.isEmpty ? String(localized: "(Removed)") : text)
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
               actionTitle: state.exists ? String(localized: "Keep \(title) File") : String(localized: "Keep File Deleted")) {
            resolution.chooseFile(state)
        }
    }

    private var hasExistenceConflict: Bool { candidate.conflicts.contains { $0.field == "exists" } }
    private var hasPathConflict: Bool { candidate.conflicts.contains { $0.field == "path" || $0.field == "path_occupied" } }

    private func text(in state: ReconciliationResourceState) -> String {
        state.exists ? state.content?.primaryText ?? "" : ""
    }
}
