import AppKit
import SwiftUI

struct DraftReconciliationView: View {
    let candidate: DraftReconciliationCandidate
    let updateRequest: Int
    let usesContextualUpdateAction: Bool
    let updateButtonTitle: String
    let initialResolution: ReconciliationResourceState
    let onResolvedStateChange: ((ReconciliationResourceState) -> Void)?
    let onUpdateStateChange: ((Bool, Bool) -> Void)?
    let onCancel: () -> Void
    let onApplied: () -> Void
    let onApply: (ReconciliationResourceState?) async throws -> Void

    @State private var resolvedExists: Bool
    @State private var resolvedPath: String
    @State private var resolvedContent: String
    @State private var isApplying = false
    @State private var errorMessage: String?
    @State private var confirmsDiscard = false
    @State private var comparison = Comparison.sharedChanges

    private enum Comparison: String, CaseIterable {
        case sharedChanges = "Shared changes"
        case yourChanges = "Your changes"
        case preview = "Result preview"
    }

    init(
        candidate: DraftReconciliationCandidate,
        updateRequest: Int = 0,
        usesContextualUpdateAction: Bool = false,
        updateButtonTitle: String = "Update",
        initialResolvedState: ReconciliationResourceState? = nil,
        onResolvedStateChange: ((ReconciliationResourceState) -> Void)? = nil,
        onUpdateStateChange: ((Bool, Bool) -> Void)? = nil,
        onCancel: @escaping () -> Void,
        onApplied: (() -> Void)? = nil,
        onApply: @escaping (ReconciliationResourceState?) async throws -> Void
    ) {
        self.candidate = candidate
        self.updateRequest = updateRequest
        self.usesContextualUpdateAction = usesContextualUpdateAction
        self.updateButtonTitle = updateButtonTitle
        self.onResolvedStateChange = onResolvedStateChange
        self.onUpdateStateChange = onUpdateStateChange
        self.onCancel = onCancel
        self.onApplied = onApplied ?? onCancel
        self.onApply = onApply
        let initial = initialResolvedState ?? candidate.proposedState ?? candidate.draftState
        self.initialResolution = initial
        _resolvedExists = State(initialValue: initial.exists)
        _resolvedPath = State(initialValue: initial.resource.path ?? "")
        _resolvedContent = State(
            initialValue: Self.resolutionContentTemplate(
                for: candidate,
                preferredState: initial
            ).primaryText
        )
    }

    var body: some View {
        VStack(spacing: 0) {
            if !candidate.valid {
                HStack(spacing: 7) {
                    Image(systemName: "arrow.trianglehead.2.clockwise.rotate.90")
                    Text("A newer shared version is available. Review the latest update again.")
                    Spacer()
                }
                .font(.caption)
                .foregroundStyle(.orange)
                .padding(.horizontal, 10)
                .frame(height: 34)
                Divider()
            }

            if candidate.status == .conflicts {
                conflictResolution
            } else {
                cleanDiff
            }

            if !usesContextualUpdateAction {
                Divider()
                HStack {
                    Button("Cancel") {
                        if hasEdits { confirmsDiscard = true } else { onCancel() }
                    }
                        .keyboardShortcut(.cancelAction)
                        .disabled(isApplying)
                    Spacer()
                    Button {
                        apply()
                    } label: {
                        if isApplying {
                            ProgressView().controlSize(.small)
                        } else {
                            Text(updateButtonTitle)
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canApply)
                }
                .padding(12)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { publishUpdateState() }
        .onChange(of: canApply) { _, _ in publishUpdateState() }
        .onChange(of: isApplying) { _, _ in publishUpdateState() }
        .onChange(of: resolvedExists) { _, _ in publishResolution() }
        .onChange(of: resolvedPath) { _, _ in publishResolution() }
        .onChange(of: resolvedContent) { _, _ in publishResolution() }
        .onChange(of: updateRequest) { _, _ in
            guard usesContextualUpdateAction else { return }
            apply()
        }
        .confirmationDialog("Discard your conflict resolution edits?", isPresented: $confirmsDiscard) {
            Button("Discard Edits", role: .destructive) { onCancel() }
            Button("Keep Editing", role: .cancel) {}
        }
        .alert(
            "Could Not Update Draft",
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

    private var canApply: Bool {
        !isApplying
            && candidate.valid
            && !(candidate.status == .conflicts && resolvedExists
                && resolvedPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }

    private var hasEdits: Bool {
        resolvedExists != initialResolution.exists
            || resolvedPath != (initialResolution.resource.path ?? "")
            || resolvedContent != Self.resolutionContentTemplate(
                for: candidate, preferredState: initialResolution
            ).primaryText
    }

    private func publishUpdateState() {
        guard usesContextualUpdateAction else { return }
        onUpdateStateChange?(canApply, isApplying)
    }

    private func publishResolution() {
        onResolvedStateChange?(resolvedState)
    }

    @ViewBuilder
    private var cleanDiff: some View {
        let states = candidate.postSyncDiffStates
        if states.base != states.draft {
            reconciliationDiff(
                from: states.base,
                to: states.draft,
                title: "Shared Version → Updated Draft"
            )
        } else {
            ContentUnavailableView(
                "No Draft Changes",
                systemImage: "doc.text",
                description: Text(
                    "Updating moves this draft to the latest shared version without leaving changes to this file."
                )
            )
        }
    }

    private var conflictResolution: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Label(conflictSummary, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)

                Spacer(minLength: 12)

                if hasExistenceConflict {
                    Toggle("Keep File", isOn: $resolvedExists)
                        .toggleStyle(.switch)
                        .controlSize(.small)
                }

                if resolvedExists && hasPathConflict {
                    TextField("Path", text: $resolvedPath)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 280)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .disabled(isApplying)
            Divider()

            VSplitView {
                VStack(spacing: 0) {
                    Picker("Compare versions", selection: $comparison) {
                        ForEach(Comparison.allCases, id: \.self) { comparison in
                            Text(comparison.rawValue).tag(comparison)
                        }
                    }
                    .pickerStyle(.segmented)
                    .padding(10)

                    switch comparison {
                    case .sharedChanges:
                        reconciliationDiff(from: candidate.baseState, to: candidate.currentState,
                                           title: "Original Version → Latest Shared Version")
                    case .yourChanges:
                        reconciliationDiff(from: candidate.baseState, to: candidate.draftState,
                                           title: "Original Version → Your Changes")
                    case .preview:
                        reconciliationDiff(from: candidate.currentState, to: resolvedState,
                                           title: "Latest Shared Version → Final Result")
                    }
                }
                .frame(minHeight: 220, maxHeight: .infinity)

                resolvedContentPane
                    .frame(minHeight: 180, maxHeight: .infinity)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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

    private var resolvedContentPane: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 3) {
                Text("Final Result").font(.caption.weight(.medium))
                Text("Compare both sets of changes, then edit the final content below.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            Divider()

            if resolvedExists {
                TextEditor(text: $resolvedContent)
                    .font(.system(.body, design: .monospaced))
                    .scrollContentBackground(.hidden)
                    .background(Color(nsColor: .textBackgroundColor))
                    .disabled(isApplying)
                    .accessibilityLabel("Final resolved content")
            } else {
                ContentUnavailableView(
                    "File Removed",
                    systemImage: "trash",
                    description: Text("The resolved result removes this file.")
                )
            }
        }
    }

    private var conflictSummary: String {
        let fields = Array(Set(candidate.conflicts.map(\.field))).sorted()
        let noun = candidate.conflicts.count == 1 ? "conflict" : "conflicts"
        guard !fields.isEmpty else { return "\(candidate.conflicts.count) \(noun)" }
        return "\(candidate.conflicts.count) \(noun): \(fields.joined(separator: ", "))"
    }

    private var hasExistenceConflict: Bool {
        candidate.conflicts.contains { $0.field == "exists" }
    }

    private var hasPathConflict: Bool {
        candidate.conflicts.contains { $0.field == "path" || $0.field == "path_occupied" }
    }

    private func text(in state: ReconciliationResourceState) -> String {
        state.exists ? state.content?.primaryText ?? "" : ""
    }

    private func path(in state: ReconciliationResourceState) -> String? {
        state.exists ? state.resource.path : nil
    }

    private func apply() {
        guard canApply else { return }
        isApplying = true
        Task {
            defer { isApplying = false }
            do {
                let resolved = candidate.status == .conflicts ? resolvedState : nil
                try await onApply(resolved)
                onApplied()
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    private var resolvedState: ReconciliationResourceState {
        let template = candidate.proposedState ?? candidate.draftState
        let contentTemplate = Self.resolutionContentTemplate(
            for: candidate,
            preferredState: template
        )
        let resource = ServerDraftResourceReference(
            scope: template.resource.scope,
            id: template.resource.id,
            path: resolvedExists ? resolvedPath : template.resource.path
        )
        return .init(
            exists: resolvedExists,
            resource: resource,
            content: resolvedExists
                ? contentTemplate.replacingPrimaryText(with: resolvedContent)
                : nil
        )
    }

    static func resolutionContentTemplate(
        for candidate: DraftReconciliationCandidate,
        preferredState: ReconciliationResourceState
    ) -> DaemonDraftContent {
        preferredState.content
            ?? candidate.currentState.content
            ?? candidate.draftState.content
            ?? candidate.baseState.content
            ?? .init(description: nil, content: "")
    }
}
