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
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text(candidate.draftState.resource.path ?? candidate.currentState.resource.path ?? "")
                        .font(.caption.monospaced()).foregroundStyle(.secondary)
                    DraftResolutionContent(candidate: candidate, resolution: $resolution)
                }
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .topLeading)
            }
            .disabled(isApplying)
            if let message = errorMessage ?? (candidate.valid ? nil : String(localized: "The remote version changed. Close this window and check the latest version again.")) {
                FormErrorMessage(message: message).padding(.horizontal, 24).padding(.vertical, 12)
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
    }

    private func apply() {
        guard !isApplying, resolution.canSave else { return }
        errorMessage = nil
        isApplying = true
        Task {
            defer { isApplying = false }
            do {
                try await onApply(candidate.status == .conflicts ? resolution.state : nil)
                onApplied()
            } catch { errorMessage = error.actionMessage }
        }
    }
}
