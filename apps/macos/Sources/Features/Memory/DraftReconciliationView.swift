import AppKit
import SwiftUI

struct DraftReconciliationView: View {
    let candidate: DraftReconciliationCandidate
    let usesContextualUpdateAction: Bool
    let updateButtonTitle: String
    let onResolutionChange: ((DraftResolution) -> Void)?
    let onCancel: () -> Void
    let onApplied: () -> Void
    let onApply: (ReconciliationResourceState?) async throws -> Void

    @State private var resolution: DraftResolution
    @State private var isApplying = false
    @State private var errorMessage: String?
    @State private var confirmsDiscard = false
    @State private var replacement: ReconciliationResourceState?

    init(candidate: DraftReconciliationCandidate,
         usesContextualUpdateAction: Bool = false,
         updateButtonTitle: String = String(localized: "Save to Draft"),
         initialResolution: DraftResolution? = nil,
         onResolutionChange: ((DraftResolution) -> Void)? = nil,
         onCancel: @escaping () -> Void,
         onApplied: (() -> Void)? = nil,
         onApply: @escaping (ReconciliationResourceState?) async throws -> Void) {
        self.candidate = candidate
        self.usesContextualUpdateAction = usesContextualUpdateAction
        self.updateButtonTitle = updateButtonTitle
        self.onResolutionChange = onResolutionChange
        self.onCancel = onCancel
        self.onApplied = onApplied ?? onCancel
        self.onApply = onApply
        _resolution = State(initialValue: initialResolution ?? DraftResolution(candidate: candidate))
    }

    var body: some View {
        VStack(spacing: 0) {
            if !candidate.valid {
                Text("The remote version changed. Close this window and check the latest version again.")
                    .font(.callout).foregroundStyle(.orange).padding(12)
            }
            if candidate.status == .conflicts {
                conflictResolution
            } else {
                cleanDiff
            }
            if !usesContextualUpdateAction {
                SheetActionBar(
                    confirmationTitle: Text(updateButtonTitle), progressTitle: "Saving…",
                    isWorking: isApplying, canConfirm: resolution.canSave,
                    confirmationIdentifier: "draft-resolution-save",
                    cancel: {
                        if resolution.hasEdits { confirmsDiscard = true } else { onCancel() }
                    }, confirm: apply
                )
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onChange(of: resolution) { _, value in onResolutionChange?(value) }
        .interactiveDismissDisabled(!usesContextualUpdateAction && (resolution.hasEdits || isApplying))
        .confirmationDialog("Discard your conflict resolution edits?", isPresented: $confirmsDiscard) {
            Button("Discard Edits", role: .destructive, action: onCancel)
            Button("Keep Editing", role: .cancel) {}
        }
        .confirmationDialog("Replace the entire result?", isPresented: Binding(
            get: { replacement != nil }, set: { if !$0 { replacement = nil } }
        ), presenting: replacement) { version in
            Button("Replace Entire Result", role: .destructive) { resolution.chooseFile(version) }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("This replaces all merged changes and edits with the selected file version.")
        }
        .alert("Could Not Save Draft", isPresented: Binding(
            get: { errorMessage != nil }, set: { if !$0 { errorMessage = nil } }
        )) {
            Button("OK") { errorMessage = nil }
        } message: {
            Text(errorMessage ?? "").textSelection(.enabled)
        }
    }

    private var conflictResolution: some View {
        VSplitView {
            if hasExistenceConflict || hasPathConflict || !resolution.sections.isEmpty
                || resolution.unresolvedFields.contains("content") {
                ScrollView {
                    DraftConflictView(candidate: candidate, resolution: $resolution).padding(16)
                }
                .frame(minHeight: 120, idealHeight: 240, maxHeight: .infinity)
                .disabled(isApplying)
            }
            resultEditor
                .frame(minHeight: 180, maxHeight: .infinity)
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var resultEditor: some View {
        VStack(spacing: 0) {
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Merged Result").font(.headline)
                    Text(resolution.canSave
                         ? "You can edit this result before saving it to the draft."
                         : "Choose a version for each change above to continue.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Menu {
                    Button("Replace Entire File with Remote…") { replacement = candidate.currentState }
                    Button("Replace Entire File with Draft…") { replacement = candidate.draftState }
                } label: { Image(systemName: "ellipsis") }
                .menuStyle(.borderlessButton).fixedSize()
                .help("Replace the entire result with one file version")
                .accessibilityLabel("Whole-file alternatives")
                .disabled(isApplying)
            }.padding(.horizontal, 16).padding(.vertical, 10)
            if hasPathConflict, resolution.state.exists {
                TextField("Final path", text: Binding(get: { resolution.path }, set: { resolution.choosePath($0) }))
                    .textFieldStyle(.roundedBorder).padding(.horizontal, 16).padding(.bottom, 12)
                    .disabled(isApplying)
            }
            Divider()
            if resolution.state.exists {
                TextEditor(text: Binding(
                    get: { resolution.canEditContent ? resolution.text : resolution.previewText },
                    set: { resolution.editContent($0) }
                ))
                .font(.system(.body, design: .monospaced))
                .scrollContentBackground(.hidden)
                .background(Color(nsColor: .textBackgroundColor))
                .disabled(isApplying || !resolution.canEditContent)
                .accessibilityLabel("Merged result")
                .frame(minHeight: 120)
            } else {
                ContentUnavailableView("File Will Be Deleted", systemImage: "trash",
                    description: Text("Saving this result keeps the file deleted in the draft."))
            }
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var hasExistenceConflict: Bool { candidate.conflicts.contains { $0.field == "exists" } }
    private var hasPathConflict: Bool { candidate.conflicts.contains { $0.field == "path" || $0.field == "path_occupied" } }

    @ViewBuilder
    private var cleanDiff: some View {
        let states = candidate.postSyncDiffStates
        if states.base != states.draft {
            reconciliationDiff(from: states.base, to: states.draft, title: String(localized: "Remote Version → Updated Draft"))
        } else {
            ContentUnavailableView("No Draft Changes", systemImage: "doc.text",
                description: Text("Saving brings this draft up to date without leaving changes to publish."))
        }
    }

    private func reconciliationDiff(
        from originalState: ReconciliationResourceState,
        to modifiedState: ReconciliationResourceState,
        title: String
    ) -> some View {
        let originalPath = path(in: originalState)
        let modifiedPath = path(in: modifiedState)
        return GeometryReader { geometry in
            ScrollView(.vertical) {
                VStack(alignment: .leading, spacing: 0) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(title)
                            .font(.caption.weight(.medium))
                        if originalPath != modifiedPath {
                            Text("\(originalPath ?? "/dev/null") → \(modifiedPath ?? "/dev/null")")
                                .font(.caption2.monospaced())
                                .foregroundStyle(.secondary)
                                .textSelection(.enabled)
                        }
                    }
                    .padding(.horizontal, 12)
                    .frame(
                        maxWidth: .infinity,
                        minHeight: originalPath == modifiedPath ? 28 : 42,
                        alignment: .leading
                    )
                    .background(Color.accentColor.opacity(0.06))

                    UnifiedDiffView(
                        presentation: UnifiedDiffPresentation(
                            model: SplitDiffModel.make(
                                original: text(in: originalState),
                                modified: text(in: modifiedState)
                            )
                        )
                    )
                }
                .frame(
                    maxWidth: .infinity,
                    minHeight: geometry.size.height,
                    alignment: .topLeading
                )
            }
        }
    }

    private func text(in state: ReconciliationResourceState) -> String {
        state.exists ? state.content?.primaryText ?? "" : ""
    }

    private func path(in state: ReconciliationResourceState) -> String? {
        state.exists ? state.resource.path : nil
    }

    private func apply() {
        guard !isApplying, resolution.canSave else { return }
        isApplying = true
        Task {
            defer { isApplying = false }
            do {
                try await onApply(candidate.status == .conflicts ? resolution.state : nil)
                onApplied()
            } catch { errorMessage = error.localizedDescription }
        }
    }
}
