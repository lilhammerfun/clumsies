import AppKit
import SwiftUI
import UniformTypeIdentifiers

enum RetrievalDiagnosticsLayout {
    static let runListMinimumWidth: CGFloat = 360
    static let runListIdealWidth: CGFloat = 360
    static let runListMaximumWidth: CGFloat = 480
    static let mainPaneMinimumWidth: CGFloat = 650
    static let dividerAllowance: CGFloat = 2
    static let minimumWindowContentWidth =
        runListMinimumWidth
        + mainPaneMinimumWidth
        + dividerAllowance
}

enum RetrievalEvidenceReviewAction: Equatable {
    case done
    case noMatch
    case confirm

    init(hasSelection: Bool, canRecordNoMatch: Bool) {
        if hasSelection {
            self = .confirm
        } else if canRecordNoMatch {
            self = .noMatch
        } else {
            self = .done
        }
    }

    var title: String {
        switch self {
        case .done: String(localized: "Done")
        case .noMatch: String(localized: "No Match")
        case .confirm: String(localized: "Confirm")
        }
    }
}

struct NativeRetrievalDiagnosticsView: View {
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @StateObject private var retrieval: RetrievalDiagnosticsModel

    init(model: @autoclosure @escaping () -> RetrievalDiagnosticsModel) {
        _retrieval = StateObject(wrappedValue: model())
    }

    var body: some View {
        RetrievalDiagnosticsView(
            model: retrieval,
            projectName: workspaceContext.activeProject?.name,
            projectId: workspaceContext.activeProjectId
        )
        .task(id: workspaceContext.activeProjectId) {
            await retrieval.load(projectId: workspaceContext.activeProjectId)
        }
    }
}

private struct RetrievalDiagnosticsView: View {
    @ObservedObject var model: RetrievalDiagnosticsModel
    let projectName: String?
    let projectId: String?

    @State private var confirmsClear = false

    var body: some View {
        NavigationSplitView {
            RetrievalRunList(
                model: model,
                scopeTitle: projectName ?? projectId ?? String(localized: "All Projects"),
                onRefresh: {
                    Task { await model.load(projectId: projectId) }
                },
                onClearHistory: {
                    confirmsClear = true
                }
            )
            .frame(
                minWidth: RetrievalDiagnosticsLayout.runListMinimumWidth,
                idealWidth: RetrievalDiagnosticsLayout.runListIdealWidth,
                maxWidth: RetrievalDiagnosticsLayout.runListMaximumWidth,
                maxHeight: .infinity
            )
            .navigationSplitViewColumnWidth(
                min: RetrievalDiagnosticsLayout.runListMinimumWidth,
                ideal: RetrievalDiagnosticsLayout.runListIdealWidth,
                max: RetrievalDiagnosticsLayout.runListMaximumWidth
            )
        } detail: {
            RetrievalRunDetailView(model: model)
                .frame(
                    minWidth: RetrievalDiagnosticsLayout.mainPaneMinimumWidth,
                    maxWidth: .infinity,
                    maxHeight: .infinity
                )
        }
        .confirmationDialog(
            "Clear unpinned retrieval history?",
            isPresented: $confirmsClear
        ) {
            Button("Clear History", role: .destructive) {
                Task { await model.clearUnpinnedHistory() }
            }
        } message: {
            Text("Runs used by Evaluation Cases will be kept.")
        }
    }
}

struct RetrievalRunDetailView: View {
    @ObservedObject var model: RetrievalDiagnosticsModel
    @State private var exportError: String?
    @State private var showsEvidenceReview = false

    var body: some View {
        RetrievalRunContent(model: model)
        .toolbar {
            if #available(macOS 26.0, *) {
                ToolbarSpacer(.flexible, placement: .automatic)
            }

            ToolbarItem(placement: .trailingPinned) {
                Menu {
                    if model.detail?.run.status == .succeeded {
                        if model.detail?.evaluationCase == nil {
                            Button {
                                Task { await model.markInaccurate() }
                            } label: {
                                Label("Report Inaccurate", systemImage: "flag")
                            }
                            .disabled(model.isMutating)
                        } else {
                            Button {
                                showsEvidenceReview = true
                            } label: {
                                Label(
                                    "Review Evidence",
                                    systemImage: "doc.text.magnifyingglass"
                                )
                            }
                        }
                    }

                    if canExportEvaluationSet {
                        if model.detail?.run.status == .succeeded {
                            Divider()
                        }

                        Button {
                            Task { await exportEvaluationSet() }
                        } label: {
                            Label(
                                "Export Evaluation Set",
                                systemImage: "square.and.arrow.up"
                            )
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                }
                .menuIndicator(.hidden)
                .toolbarHelp(String(localized: "Retrieval Run Actions"))
                .accessibilityLabel("Retrieval Run Actions")
                .disabled(!hasMoreActions)
            }
        }
        .sheet(isPresented: $showsEvidenceReview) {
            RetrievalEvidenceReviewSheet(
                model: model,
                isPresented: $showsEvidenceReview
            )
        }
        .onChange(of: model.selectedRunId) { _, _ in
            showsEvidenceReview = false
            exportError = nil
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            if (model.detail != nil || model.selectedRunId == nil),
               let message = model.errorMessage ?? exportError {
                VStack(spacing: 0) {
                    HStack(spacing: 8) {
                        Image(systemName: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                        Text(message)
                            .textSelection(.enabled)
                        Spacer()
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    Divider()
                }
            }
        }
    }

    private var canExportEvaluationSet: Bool {
        model.detail?.evaluationCase?.status == .ready
            || model.runs.contains { $0.evaluationCaseStatus == .ready }
    }

    private var hasMoreActions: Bool {
        model.detail?.run.status == .succeeded || canExportEvaluationSet
    }

    @MainActor
    private func exportEvaluationSet() async {
        exportError = nil
        do {
            let exported = try await model.exportEvaluationSet()
            let panel = NSSavePanel()
            panel.nameFieldStringValue = "clumsies-retrieval-evaluation.json"
            panel.allowedContentTypes = [.json]
            guard await panel.selectionResponse == .OK, let url = panel.url else { return }
            try exported.fixtureJson.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            exportError = error.localizedDescription
        }
    }
}

private struct RetrievalRunList: View {
    @ObservedObject var model: RetrievalDiagnosticsModel
    let scopeTitle: String
    let onRefresh: () -> Void
    let onClearHistory: () -> Void
    @State private var selectedRunId: String?

    var body: some View {
        ScrollView(.vertical) {
            LazyVStack(alignment: .leading, spacing: 2) {
                Text(scopeTitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.bottom, 2)

                ForEach(model.runs) { run in
                    let isSelected = selectedRunId == run.runId
                    Button {
                        selectedRunId = run.runId
                    } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(run.query)
                                .lineLimit(2)
                                .truncationMode(.tail)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            HStack(spacing: 6) {
                                Image(systemName: run.status.symbolName)
                                    .foregroundStyle(isSelected ? Color.white : run.status.tint)
                                Text(run.createdAt)
                                    .lineLimit(1)
                                if let evaluationStatus = run.evaluationCaseStatus {
                                    Image(systemName: evaluationStatus.symbolName)
                                        .help(evaluationStatus.helpText)
                                }
                            }
                            .font(.caption)
                            .foregroundStyle(
                                isSelected ? Color.white.opacity(0.8) : Color.secondary
                            )
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .foregroundStyle(isSelected ? Color.white : Color.primary)
                        .background {
                            if isSelected {
                                RoundedRectangle(cornerRadius: 6, style: .continuous)
                                    .fill(Color.accentColor)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                    .focusEffectDisabled()
                    .padding(.horizontal, 8)
                }
                if model.nextCursor != nil {
                    Button {
                        Task { await model.loadMore() }
                    } label: {
                        if model.isLoadingMore {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Text("Load More")
                        }
                    }
                    .buttonStyle(.borderless)
                    .frame(maxWidth: .infinity)
                }
            }
            .padding(.vertical, 8)
        }
        .overlay {
            if model.isLoading && model.runs.isEmpty {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if model.runs.isEmpty {
                ContentUnavailableView(
                    "No Retrieval Runs",
                    systemImage: "text.magnifyingglass",
                    description: Text("Memory activation results will appear here.")
                )
            }
        }
        .toolbar {
            ToolbarItemGroup(placement: .navigation) {
                Button(action: onRefresh) {
                    Image(systemName: "arrow.clockwise")
                }
                .toolbarHelp(String(localized: "Refresh Retrieval Runs"))
                .accessibilityLabel("Refresh Retrieval Runs")

                Button(role: .destructive, action: onClearHistory) {
                    Image(systemName: "trash")
                }
                .toolbarHelp(String(localized: "Clear Unpinned Retrieval History"))
                .accessibilityLabel("Clear Unpinned Retrieval History")
                .disabled(model.runs.isEmpty)
            }
        }
        .onChange(of: selectedRunId) { _, runId in
            guard runId != model.selectedRunId else { return }
            DispatchQueue.main.async {
                guard runId != model.selectedRunId else { return }
                Task { await model.select(runId: runId) }
            }
        }
        .onChange(of: model.selectedRunId) { _, runId in
            guard selectedRunId != runId else { return }
            selectedRunId = runId
        }
    }
}

private struct RetrievalRunContent: View {
    @ObservedObject var model: RetrievalDiagnosticsModel

    var body: some View {
        if model.isLoading, model.detail == nil {
            ProgressView()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let detail = model.detail {
            VStack(spacing: 0) {
                RetrievalRunSummary(run: detail.run)
                Divider()
                if let error = detail.run.errorSummary {
                    VStack(spacing: 0) {
                        HStack(alignment: .top, spacing: 8) {
                            Image(systemName: "exclamationmark.triangle")
                                .foregroundStyle(.red)
                            Text(error)
                                .textSelection(.enabled)
                            Spacer()
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                        Divider()
                    }
                }
                CandidateTraceTable(detail: detail)
                    .id(detail.run.runId)
            }
        } else if model.selectedRunId != nil {
            ContentUnavailableView {
                Label("Retrieval Run Unavailable", systemImage: "exclamationmark.triangle")
            } description: {
                Text(model.errorMessage ?? String(localized: "This retrieval record may have been removed."))
                    .textSelection(.enabled)
            } actions: {
                Button("Retry") {
                    Task { await model.select(runId: model.selectedRunId) }
                }
            }
        } else {
            ContentUnavailableView(
                "Select a Retrieval Run",
                systemImage: "list.bullet.rectangle"
            )
        }
    }
}

private struct RetrievalRunSummary: View {
    let run: RetrievalRun

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(run.query)
                    .font(.headline)
                    .textSelection(.enabled)
                    .lineLimit(2)
                Spacer()
                Label(run.status.title, systemImage: run.status.symbolName)
                    .font(.caption)
                    .foregroundStyle(run.status.tint)
            }
            HStack(spacing: 24) {
                summaryItem(
                    String(localized: "Returned"),
                    String(localized: "\(run.returnedFragmentCount) fragments · \(run.returnedTokenCount) tokens")
                )
                .help("Includes reused chunks already available to the agent. Tokens describe the selected content, not newly sent tokens or model usage.")
                summaryItem(String(localized: "Corpus"), String(localized: "\(run.resourceCount) resources · \(run.unitCount) units"))
                summaryItem(String(localized: "Total"), formatDuration(run.latencies.totalUs))
            }
        }
        .padding(16)
    }

    @ViewBuilder
    private func summaryItem(_ label: String, _ value: String) -> some View {
        HStack(spacing: 5) {
            Text(label)
                .foregroundStyle(.secondary)
            Text(value)
                .textSelection(.enabled)
                .lineLimit(1)
        }
        .font(.caption)
    }
}

enum RetrievalCandidateFilter: String, CaseIterable, Identifiable {
    case all = "All"
    case selected = "Selected"
    case excluded = "Not Selected"

    var id: Self { self }

    var title: String {
        switch self {
        case .all: String(localized: "All")
        case .selected: String(localized: "Selected")
        case .excluded: String(localized: "Not Selected")
        }
    }


    func includes(_ candidate: RetrievalCandidate) -> Bool {
        switch self {
        case .all: true
        case .selected: candidate.selected
        case .excluded: !candidate.selected
        }
    }
}

struct RetrievalCandidateSort: SortComparator {
    enum Column: CaseIterable {
        case final, bm25, vector, rrf, rerank

        func rank(_ candidate: RetrievalCandidate) -> UInt64? {
            switch self {
            case .final: candidate.finalRank
            case .bm25: candidate.bm25Rank ?? candidate.exactRank
            case .vector: candidate.vectorRank
            case .rrf: candidate.rrfRank
            case .rerank: candidate.rerankerRank
            }
        }
    }

    let column: Column
    var order: SortOrder = .forward

    func compare(_ lhs: RetrievalCandidate, _ rhs: RetrievalCandidate) -> ComparisonResult {
        switch (column.rank(lhs), column.rank(rhs)) {
        case (nil, nil): .orderedSame
        case (nil, _): .orderedDescending
        case (_, nil): .orderedAscending
        case let (left?, right?):
            left == right ? .orderedSame
                : (left < right) == (order == .forward) ? .orderedAscending : .orderedDescending
        }
    }
}

private struct CandidateTraceTable: View {
    let detail: RetrievalRunDetail
    @State private var filter: RetrievalCandidateFilter = .all
    @State private var sortOrder = [RetrievalCandidateSort(column: .final)]
    @State private var showsResultHelp = false

    private var candidates: [RetrievalCandidate] {
        detail.candidates.filter(filter.includes).sorted(using: sortOrder)
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Candidate Trace")
                    .font(.headline)
                Picker("Candidates", selection: $filter) {
                    ForEach(RetrievalCandidateFilter.allCases) { filter in
                        Text(filter.title).tag(filter)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                Spacer()
                Text("\(candidates.count) / \(detail.candidates.count)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 16)
            .frame(height: 42)
            Divider()
            Table(candidates, sortOrder: $sortOrder) {
                TableColumn("Final", sortUsing: RetrievalCandidateSort(column: .final)) { candidate in
                    Text(rank(candidate.finalRank))
                        .monospacedDigit()
                }
                .width(44)
                TableColumn("Resource") { candidate in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(candidate.path)
                            .lineLimit(1)
                        if !candidate.headingPath.isEmpty {
                            Text(candidate.headingPath.joined(separator: " › "))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }
                    }
                    .help(candidate.evidenceExcerpt)
                }
                .width(min: 150, ideal: 220)
                TableColumn("BM25", sortUsing: RetrievalCandidateSort(column: .bm25)) { candidate in
                    stage(rank: candidate.bm25Rank ?? candidate.exactRank, score: candidate.bm25Score)
                }
                .width(64)
                TableColumn("Vector", sortUsing: RetrievalCandidateSort(column: .vector)) { candidate in
                    stage(rank: candidate.vectorRank, score: candidate.vectorScore)
                }
                .width(64)
                TableColumn("RRF", sortUsing: RetrievalCandidateSort(column: .rrf)) { candidate in
                    stage(rank: candidate.rrfRank, score: candidate.rrfScore)
                }
                .width(64)
                TableColumn("Rerank", sortUsing: RetrievalCandidateSort(column: .rerank)) { candidate in
                    stage(rank: candidate.rerankerRank, score: candidate.rerankerRelevance)
                }
                .width(64)
                TableColumn("Result") { candidate in
                    Text(result(candidate))
                        .foregroundStyle(candidate.selected ? .primary : .secondary)
                        .help(candidate.selected
                              ? candidate.deltaAction?.explanation ?? RetrievalExclusionReason.selected.explanation
                              : candidate.exclusionReason.explanation)
                }
                .width(min: 100, ideal: 120)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .overlay {
                if candidates.isEmpty {
                    ContentUnavailableView(
                        detail.candidates.isEmpty ? "No Candidates" : "No Matching Candidates",
                        systemImage: "list.bullet.rectangle"
                    )
                }
            }
            .overlay(alignment: .topTrailing) {
                Button {
                    showsResultHelp = true
                } label: {
                    Image(systemName: "info.circle")
                        .frame(width: 24, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .accessibilityLabel("About Results")
                .help("About Results")
                .padding(.trailing, 8)
                .popover(isPresented: $showsResultHelp, arrowEdge: .trailing) {
                    VStack(alignment: .leading, spacing: 12) {
                        Text("About Results").font(.headline)
                        Text("Result shows how a selected chunk is passed to the agent, or why a candidate was not selected.")
                            .font(.callout)
                        ScrollView {
                            VStack(alignment: .leading, spacing: 12) {
                                ForEach(RetrievalDeltaAction.allCases, id: \.self) { action in
                                    resultDefinition(action.title, action.explanation)
                                }
                                Divider()
                                ForEach(RetrievalExclusionReason.allCases, id: \.self) { reason in
                                    resultDefinition(reason.label, reason.explanation)
                                }
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .frame(maxHeight: 480)
                    }
                    .padding(16)
                    .frame(width: 380)
                }
            }
        }
    }

    private func resultDefinition(_ title: String, _ explanation: String) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title).fontWeight(.medium)
            Text(explanation).foregroundStyle(.secondary)
        }
        .font(.callout)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func stage(rank: UInt64?, score: Double?) -> some View {
        VStack(alignment: .trailing, spacing: 1) {
            Text(self.rank(rank))
                .monospacedDigit()
            if let score {
                Text(score.formatted(.number.precision(.fractionLength(3))))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
        }
    }

    private func rank(_ rank: UInt64?) -> String {
        rank.map { "#\($0)" } ?? "—"
    }

    private func result(_ candidate: RetrievalCandidate) -> String {
        candidate.selected
            ? candidate.deltaAction?.title ?? String(localized: "Selected")
            : candidate.exclusionReason.label
    }
}

private struct RetrievalEvidenceReviewSheet: View {
    @ObservedObject var model: RetrievalDiagnosticsModel
    @Binding var isPresented: Bool

    private var action: RetrievalEvidenceReviewAction {
        RetrievalEvidenceReviewAction(
            hasSelection: !model.evidenceDrafts.isEmpty,
            canRecordNoMatch: model.detail?.evaluationCase?.status == .draft
        )
    }

    var body: some View {
        if let detail = model.detail {
            VStack(spacing: 0) {
                HStack {
                    Text("Review Evidence")
                        .font(.headline)
                    Spacer()
                }
                .padding(16)

                Divider()

                if detail.evidenceSuggestions.isEmpty {
                    ContentUnavailableView(
                        "No Suggested Evidence",
                        systemImage: "doc.text.magnifyingglass"
                    )
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    List(detail.evidenceSuggestions) { suggestion in
                        Toggle(
                            isOn: Binding(
                                get: { model.isEvidenceSelected(suggestion) },
                                set: {
                                    model.setEvidenceSelected($0, suggestion: suggestion)
                                }
                            )
                        ) {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(suggestion.path)
                                    .lineLimit(1)
                                if !suggestion.headingPath.isEmpty {
                                    Text(suggestion.headingPath.joined(separator: " › "))
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                                Text(suggestion.evidenceExcerpt)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(3)
                                Label(
                                    suggestion.diagnosis,
                                    systemImage: suggestion.likelyFailureStage.symbolName
                                )
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            }
                            .padding(.vertical, 4)
                        }
                        .toggleStyle(.checkbox)
                        .disabled(model.isMutating)
                    }
                }

                Divider()

                HStack {
                    Spacer()

                    Button("Cancel") {
                        model.resetEvidenceSelection()
                        isPresented = false
                    }
                    .keyboardShortcut(.cancelAction)

                    Button(action.title) {
                        Task {
                            if action != .done {
                                guard await model.resolveEvidenceReview() else { return }
                            }
                            isPresented = false
                        }
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(model.isMutating)
                }
                .padding(16)
            }
            .frame(minWidth: 620, minHeight: 560)
            .onAppear {
                model.resetEvidenceSelection()
            }
        } else {
            ProgressView()
                .frame(width: 620, height: 560)
        }
    }
}

private extension RetrievalRunStatus {
    var symbolName: String {
        switch self {
        case .running: "clock"
        case .succeeded: "checkmark.circle.fill"
        case .failed: "exclamationmark.circle.fill"
        }
    }

    var tint: Color {
        switch self {
        case .running: .secondary
        case .succeeded: .green
        case .failed: .red
        }
    }
}

private extension EvaluationCaseStatus {
    var symbolName: String {
        switch self {
        case .draft: "sparkles"
        case .needsEvidence: "questionmark.circle"
        case .ready: "checkmark.seal.fill"
        }
    }

    var helpText: String {
        switch self {
        case .draft: String(localized: "Evidence suggestions are awaiting confirmation")
        case .needsEvidence: String(localized: "The suggested evidence did not match")
        case .ready: String(localized: "Ready for the Evaluation Set")
        }
    }

    var tint: Color {
        switch self {
        case .draft, .needsEvidence: .secondary
        case .ready: .green
        }
    }
}

private extension RetrievalFailureStage {
    var symbolName: String {
        switch self {
        case .fusion: "arrow.triangle.merge"
        case .reranking: "arrow.up.arrow.down"
        case .assembly: "line.3.horizontal.decrease"
        }
    }
}

private extension EvaluationEvidenceSuggestion {
    var diagnosis: String {
        switch likelyFailureStage {
        case .fusion:
            String(localized: "Likely lost during hybrid fusion")
        case .reranking:
            String(localized: "Likely rejected during reranking")
        case .assembly:
            String(localized: "Likely excluded by \(exclusionReason.label)")
        }
    }
}

private extension RetrievalExclusionReason {
    var explanation: String {
        switch self {
        case .selected:
            String(localized: "Selected for this retrieval, but this record does not specify Add, Replace, or Reuse.")
        case .belowRelevance:
            String(localized: "Not selected because reranking relevance fell below the cutoff.")
        case .overlap:
            String(localized: "Not selected because its source range overlaps a selected chunk from the same resource.")
        case .perResourceLimit:
            String(localized: "Not selected because this resource already contributed the maximum number of chunks.")
        case .tokenBudget:
            String(localized: "Not selected because selection stopped at the token budget.")
        case .fragmentLimit:
            String(localized: "Not selected because the maximum number of chunks was reached.")
        case .notReranked:
            String(localized: "No reranking result was recorded. The candidate may be outside the reranking shortlist, or the run may have stopped before reranking finished. This does not mean low relevance.")
        }
    }

    var label: String {
        switch self {
        case .selected: String(localized: "Selected")
        case .belowRelevance: String(localized: "Below Relevance")
        case .overlap: String(localized: "Overlap")
        case .perResourceLimit: String(localized: "Per Resource Limit")
        case .tokenBudget: String(localized: "Token Budget")
        case .fragmentLimit: String(localized: "Fragment Limit")
        case .notReranked: String(localized: "Not Reranked")
        }
    }
}

private extension RetrievalDeltaAction {
    var explanation: String {
        switch self {
        case .add:
            String(localized: "Selected and sent as new content: this chunk was not in the agent's supplied memory state.")
        case .replace:
            String(localized: "Selected and sent with updated content: this chunk changed since the agent last received it.")
        case .reuse:
            String(localized: "Selected, but its unchanged content is already in the agent's supplied memory state, so the body is not sent again.")
        }
    }
}

private func formatDuration(_ microseconds: UInt64) -> String {
    if microseconds >= 1_000_000 {
        return (Double(microseconds) / 1_000_000)
            .formatted(.number.precision(.fractionLength(2))) + " s"
    }
    if microseconds >= 1_000 {
        return (Double(microseconds) / 1_000)
            .formatted(.number.precision(.fractionLength(1))) + " ms"
    }
    return String(localized: "\(microseconds) µs")
}

private extension NSSavePanel {
    var selectionResponse: NSApplication.ModalResponse {
        get async {
            await withCheckedContinuation { continuation in
                begin { continuation.resume(returning: $0) }
            }
        }
    }
}
