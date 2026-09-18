import AppKit
import MarkdownUI
import SwiftUI

/// Content column: the agent activity this project has produced, newest first.
struct RecallSessionList: View {
    @ObservedObject var model: RecallModel

    var body: some View {
        Group {
            if model.sessions.isEmpty, let error = model.errorMessage {
                ContentUnavailableView {
                    Label("Activity Unavailable", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(error)
                } actions: {
                    Button("Try Again") { Task { await model.load() } }
                }
            } else if model.sessions.isEmpty && (model.isLoading || !model.hasLoaded) {
                ContentLoadingView(title: "Loading Activity…")
            } else if model.sessions.isEmpty {
                ContentUnavailableView(
                    model.selectedProjectId == nil ? "No Activity Yet" : "No Activity for This Project",
                    systemImage: "bubble.left.and.bubble.right",
                    description: Text(
                        model.selectedProjectId == nil
                            ? "Agent activity from bound projects appears here with user requests and recalled memory."
                            : "Try another project or choose All Projects."
                    )
                )
            } else {
                List(selection: $model.selectedSessionId) {
                    ForEach(model.sessions) { session in
                        RecallSessionRow(session: session)
                            .tag(session.id)
                    }
                    if let error = model.pageError {
                        VStack(alignment: .leading, spacing: 6) {
                            Text(error).foregroundStyle(.secondary)
                            Button("Try Again") { Task { await model.loadMoreSessions() } }
                            Button("Refresh Activity") { Task { await model.load() } }
                        }
                        .font(.caption)
                    } else if let cursor = model.nextCursor {
                        ProgressView()
                            .controlSize(.small)
                            .frame(maxWidth: .infinity)
                            .accessibilityLabel("More activity")
                            .task(id: cursor) { await model.loadMoreSessions() }
                    }
                }
                .listStyle(.inset)
                .scrollContentBackground(.hidden)
            }
        }
        .background(Color(nsColor: .controlBackgroundColor))
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if !model.sessions.isEmpty {
                if let error = model.errorMessage {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(error).foregroundStyle(.secondary)
                        Button("Try Again") { Task { await model.load() } }
                    }
                    .font(.caption)
                    .padding(8)
                }
            }
        }
    }
}

private struct RecallSessionRow: View {
    let session: RecallSessionSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(session.activityDisplayTitle)
                .fontWeight(.medium)
                .lineLimit(1)
            HStack(spacing: 6) {
                Text(session.host.activityTitle)
                if let createdAt = session.createdAt {
                    Text("·")
                    Text(Self.date(createdAt))
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }

    private static func date(_ millis: Int64) -> String {
        Date(timeIntervalSince1970: TimeInterval(millis) / 1000)
            .formatted(date: .abbreviated, time: .shortened)
    }
}

/// Detail column: user requests and the memory the agent recalled for them.
struct RecallSessionDetail: View {
    @ObservedObject var model: RecallModel

    var body: some View {
        NavigationStack {
            Group {
                if let session = model.selectedSession {
                    if session.tasks.isEmpty {
                        ContentUnavailableView(
                            "No User Requests",
                            systemImage: "text.bubble",
                            description: Text("No user requests were found in this activity.")
                        )
                    } else {
                        taskList(session)
                            .navigationTitle("Activity")
                    }
                } else if let summary = model.selectedSummary {
                    VStack(alignment: .leading, spacing: 16) {
                        Text(summary.activityDisplayTitle).font(.title2.weight(.semibold))
                        if let error = model.detailError {
                            Text(error).foregroundStyle(.secondary)
                            Button("Try Again") { Task { await model.loadSelectedSession() } }
                            Button("Refresh Activity") { Task { await model.load() } }
                            Spacer()
                        } else {
                            ContentLoadingView(title: "Activity details")
                        }
                    }
                    .padding(24)
                } else {
                    ContentUnavailableView(
                        "Select an Activity",
                        systemImage: "sidebar.left",
                        description: Text("Choose an item to see its requests and recalled memory.")
                    )
                }
            }
        }
        .id(model.selectedSessionId)
        .task(id: model.selectedSummary?.sessionToken) { await model.loadSelectedSession() }
    }

    private func taskList(_ session: RecallSession) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                RecallSessionHeader(session: session, totalTasks: model.totalTasks)

                ForEach(Array(session.tasks.enumerated()), id: \.element.id) { index, task in
                    Divider()
                    RecallTaskSection(
                        number: index + 1,
                        task: task,
                        session: session,
                        model: model
                    )
                }
                if let error = model.detailError {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(error).foregroundStyle(.secondary)
                        Button("Try Again") {
                            Task { await model.retryDetail() }
                        }
                        Button("Refresh Activity") { Task { await model.load() } }
                    }
                    .padding(.vertical)
                } else if let offset = model.nextTaskOffset {
                    ProgressView()
                        .controlSize(.small)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical)
                        .accessibilityLabel("More requests")
                        .task(id: offset) { await model.loadMoreTasks() }
                } else if model.isLoadingDetail {
                    ProgressView().controlSize(.small).padding(.vertical)
                }
            }
            .frame(maxWidth: 760, alignment: .leading)
            .padding(.horizontal, 24)
            .padding(.bottom, 32)
            .frame(maxWidth: .infinity, alignment: .top)
        }
        .background(Color(nsColor: .textBackgroundColor))
    }
}

private struct RecallSessionHeader: View {
    let session: RecallSession

    let totalTasks: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(session.activityDisplayTitle)
                .font(.title2.weight(.semibold))
                .lineLimit(3)
                .textSelection(.enabled)

            HStack(spacing: 6) {
                Text(session.host.activityTitle)
                if let createdAt = session.createdAt {
                    Text("·")
                    Text(Self.date(createdAt))
                }
            }
            .foregroundStyle(.secondary)

            Label(
                "\(totalTasks) request\(totalTasks == 1 ? "" : "s")",
                systemImage: "text.bubble"
            )
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .padding(.vertical, 24)
    }

    private static func date(_ millis: Int64) -> String {
        Date(timeIntervalSince1970: TimeInterval(millis) / 1000)
            .formatted(date: .abbreviated, time: .shortened)
    }
}

private struct RecallTaskSection: View {
    let number: Int
    let task: RecallTask
    let session: RecallSession
    let model: RecallModel

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .firstTextBaseline) {
                Text("Request \(number)")
                    .font(.headline)
                Spacer()
                if let time = task.time {
                    Text(Self.date(time))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            VStack(alignment: .leading, spacing: 5) {
                Label("User request", systemImage: "person.crop.circle")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                Text(task.text)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            if task.activations.isEmpty {
                Label(
                    "The agent did not ask Clumsies for memory while handling this request.",
                    systemImage: "brain"
                )
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(task.activations) { activation in
                    RecallActivationRow(
                        activation: activation,
                        workspaceRoot: session.workspaceRoot,
                        model: model,
                        onOpenRetrieval: {
                            model.openRetrieval(
                                session: session,
                                task: task,
                                requestNumber: number,
                                activation: activation
                            )
                        }
                    )
                }
            }
        }
        .padding(.vertical, 24)
    }

    private static func date(_ millis: Int64) -> String {
        Date(timeIntervalSince1970: TimeInterval(millis) / 1000)
            .formatted(date: .omitted, time: .shortened)
    }
}

private struct RecallActivationRow: View {
    let activation: RecallActivation
    let workspaceRoot: String
    let model: RecallModel
    let onOpenRetrieval: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Label("Memory search", systemImage: "sparkle.magnifyingglass")
                    .fontWeight(.medium)
                Spacer()
                if let status = Self.visibleStatusTitle(activation.runStatus) {
                    Text(status)
                        .font(.caption)
                        .foregroundStyle(status == "Failed" ? Color.orange : Color.secondary)
                }
            }

            VStack(alignment: .leading, spacing: 3) {
                Text("Agent query")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                Text(activation.query)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack(spacing: 10) {
                if !activation.fragments.isEmpty {
                    Text("\(activation.fragments.count) selected chunks")
                }
                if let totalUs = activation.totalUs {
                    Text(Duration.microseconds(Int64(clamping: totalUs)).formatted(.units(allowed: [.seconds, .milliseconds], width: .abbreviated)))
                }
                Spacer()
                if activation.runId != nil {
                    Button("Retrieval Process", systemImage: "chevron.right", action: onOpenRetrieval)
                        .buttonStyle(.borderless)
                        .accessibilityLabel("View retrieval process for \(activation.query)")
                } else {
                    Text("Retrieval record unavailable")
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)

            if let error = activation.resultError {
                Label("Memory search failed", systemImage: "exclamationmark.triangle")
                    .font(.callout)
                    .foregroundStyle(.orange)
                DisclosureGroup("Show technical details") {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .padding(.top, 4)
                }
                .font(.caption)
            }

            if activation.fragments.isEmpty {
                Label(
                    "No matching memory chunks were returned.",
                    systemImage: "doc.text.magnifyingglass"
                )
                .font(.caption)
                .foregroundStyle(.secondary)
            } else {
                VStack(spacing: 8) {
                    ForEach(activation.fragments) { fragment in
                        RecallFragmentRow(
                            fragment: fragment,
                            workspaceRoot: workspaceRoot,
                            runId: activation.runId,
                            model: model
                        )
                    }
                }
            }
        }
    }

    private static func visibleStatusTitle(_ status: String?) -> String? {
        switch status?.lowercased() {
        case nil, "succeeded", "success", "completed": nil
        case "running": "Searching"
        case "failed", "error": "Failed"
        case let status?: status.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }
}

struct RecallFragmentRow: View {
    let fragment: RecallFragment
    let workspaceRoot: String
    let runId: String?
    let model: RecallModel
    @State private var fullFragment: RecallFragment?
    @State private var isLoading = false
    @State private var loadFailed = false
    @State private var loadGeneration = UUID()

    private var displayedFragment: RecallFragment { fullFragment ?? fragment }

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(fragment.displayTitle)
                        .font(.headline)
                    Text(fragment.locationTitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    HStack(spacing: 8) {
                        Text(fragment.scopeTitle)
                        if let rank = fragment.finalRank {
                            Text("Result \(rank)")
                        }
                        if let delivery = fragment.deliveryTitle {
                            Text(delivery)
                        }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
                .textSelection(.enabled)

                if !displayedFragment.content.isEmpty {
                    Markdown(displayedFragment.content)
                        .markdownTheme(.gitHub)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    Text(displayedFragment.emptyContentExplanation)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }

                if isLoading {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Loading the recorded chunk…")
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                } else if loadFailed {
                    HStack(alignment: .firstTextBaseline) {
                        Text("The full retrieval record is unavailable. Showing the recorded preview.")
                        Spacer()
                        Button("Try Again") { Task { await loadFullFragment() } }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                } else if displayedFragment.truncated {
                    Text("Only the recorded preview is available for this chunk.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(8)
        }
        .task(id: runId) { await loadFullFragment() }
    }

    private func loadFullFragment() async {
        guard fullFragment == nil, let runId,
              fragment.truncated || fragment.content.isEmpty else { return }
        let generation = UUID()
        loadGeneration = generation
        isLoading = true
        loadFailed = false
        defer {
            if loadGeneration == generation { isLoading = false }
        }
        do {
            let loaded = try await model.loadFragment(
                workspaceRoot: workspaceRoot,
                runId: runId,
                unitKey: fragment.unitKey
            )
            try Task.checkCancellation()
            guard loadGeneration == generation else { return }
            fullFragment = loaded
        } catch is CancellationError {
            return
        } catch {
            if loadGeneration == generation { loadFailed = true }
        }
    }
}

struct RecallRetrievalDetail: View {
    let selection: RecallRetrievalSelection
    let onBack: () -> Void
    @StateObject private var retrieval: RetrievalDiagnosticsModel

    init(selection: RecallRetrievalSelection, daemon: DaemonXPCClient, onBack: @escaping () -> Void) {
        self.selection = selection
        self.onBack = onBack
        _retrieval = StateObject(wrappedValue: RetrievalDiagnosticsModel(daemon: daemon))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 8) {
                Button("Back to Activity", systemImage: "chevron.left", action: onBack)
                    .buttonStyle(.borderless)
                    .keyboardShortcut("[", modifiers: .command)
                Text("\(selection.sessionTitle) / Request \(selection.requestNumber)")
                    .font(.headline)
                    .lineLimit(1)
                Text(selection.requestText)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .help(selection.requestText)
                    .textSelection(.enabled)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(16)
            Divider()
            RetrievalRunDetailView(model: retrieval)
        }
        .background(Color(nsColor: .textBackgroundColor))
        .navigationTitle("Retrieval Process")
        .task(id: selection.runId) { await retrieval.select(runId: selection.runId) }
    }
}

extension RecallFragment {
    var displayTitle: String {
        headingPath.last ?? (path as NSString).lastPathComponent
    }

    var locationTitle: String {
        path
    }

    var deliveryTitle: String? {
        switch action?.lowercased() {
        case "add": "Sent to agent"
        case "replace": "Updated for agent"
        case "reuse": "Already available"
        default: nil
        }
    }

    fileprivate var emptyContentExplanation: String {
        action?.lowercased() == "reuse"
            ? "The agent already had this unchanged memory chunk, so its text was not sent again."
            : "The full text was not preserved in this activity record."
    }

    fileprivate var scopeTitle: String {
        switch scope {
        case .org?: "Shared memory"
        case .project?: "Project memory"
        case nil: "Memory"
        }
    }
}

extension RecallSession {
    var activityDisplayTitle: String {
        if let title, !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return title
        }
        return tasks.lazy
            .map(\.text)
            .first { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            ?? "Agent activity"
    }
}

private extension AgentHost {
    var activityTitle: String {
        switch self {
        case .codex: "Codex"
        case .dsh: "DSH"
        case .claudeCode: "Claude Code"
        case .opencode: "OpenCode"
        case .antigravity: "Antigravity"
        case .manual: "Manual"
        case .zed: "Zed"
        case .unknown: "Unknown"
        }
    }
}
