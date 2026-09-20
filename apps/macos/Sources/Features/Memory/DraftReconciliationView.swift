import AppKit
import SwiftUI

struct DraftReconciliationView: View {
    let candidate: DraftReconciliationCandidate
    let updateRequest: Int
    let usesContextualUpdateAction: Bool
    let updateButtonTitle: String
    let conflictMarkerLength: Int?
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
    @State private var comparison = Comparison.resolve

    private enum Comparison: String, CaseIterable {
        case resolve = "Resolve"
        case remoteChanges = "Remote changes"
        case draftChanges = "Draft changes"
        case preview = "Merge preview"
    }

    init(
        candidate: DraftReconciliationCandidate,
        updateRequest: Int = 0,
        usesContextualUpdateAction: Bool = false,
        updateButtonTitle: String = "Update",
        conflictMarkerLength: Int? = nil,
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
        self.conflictMarkerLength = conflictMarkerLength
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
                    Text("A newer remote version is available. Check the latest version again.")
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
                title: "Remote Version → Updated Draft"
            )
        } else {
            ContentUnavailableView(
                "No Draft Changes",
                systemImage: "doc.text",
                description: Text(
                    "Updating brings this draft up to date without leaving any changes to publish."
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

            Picker("Compare versions", selection: $comparison) {
                ForEach(Comparison.allCases, id: \.self) { comparison in
                    Text(comparison.rawValue).tag(comparison)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(10)
            switch comparison {
            case .resolve:
                resolvedContentPane
            case .remoteChanges:
                reconciliationDiff(from: candidate.baseState, to: candidate.currentState,
                                   title: "Draft's Starting Version → Remote Version")
            case .draftChanges:
                reconciliationDiff(from: candidate.baseState, to: candidate.draftState,
                                   title: "Draft's Starting Version → Draft Version")
            case .preview:
                reconciliationDiff(from: candidate.currentState, to: resolvedState,
                                   title: "Remote Version → Merged Result")
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
                Text("Choose between the Remote and Draft versions, or edit the merged result below.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                HStack {
                    Button("Use Remote Version") { use(candidate.currentState) }
                    Button("Use Draft Version") { use(candidate.draftState) }
                }
                .controlSize(.small)
                .disabled(isApplying)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            Divider()

            if let length = conflictMarkerLength {
                let sections = ContentConflictSection.parse(resolvedContent, markerLength: length)
                if !sections.isEmpty {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 12) {
                            ForEach(Array(sections.enumerated()), id: \.element.id) { index, section in
                                VStack(alignment: .leading, spacing: 8) {
                                    Text("Conflict \(index + 1)").font(.caption.weight(.semibold))
                                    HStack(alignment: .top, spacing: 16) {
                                        conflictChoice("Use Remote Change", text: section.shared, section: section)
                                        conflictChoice("Use Draft Change", text: section.proposed, section: section)
                                    }
                                }
                            }
                        }.padding(10)
                    }
                    .frame(maxHeight: 220)
                    Divider()
                }
            }
            if resolvedExists {
                TextEditor(text: $resolvedContent)
                    .font(.system(.body, design: .monospaced))
                    .scrollContentBackground(.hidden)
                    .background(Color(nsColor: .textBackgroundColor))
                    .disabled(isApplying)
                    .accessibilityLabel("Final resolved content")
                    .frame(minHeight: 120)
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

    private func use(_ state: ReconciliationResourceState) {
        resolvedExists = state.exists
        resolvedPath = state.resource.path ?? ""
        resolvedContent = Self.resolutionContentTemplate(for: candidate, preferredState: state).primaryText
    }

    private func conflictChoice(_ title: String, text: String, section: ContentConflictSection) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(text.isEmpty ? "(Removed)" : text)
                .font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            Button(title) {
                let source = resolvedContent as NSString
                guard NSMaxRange(section.range) <= source.length else { return }
                resolvedContent = source.replacingCharacters(in: section.range, with: text)
            }.disabled(isApplying)
        }.frame(maxWidth: .infinity, alignment: .leading)
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
