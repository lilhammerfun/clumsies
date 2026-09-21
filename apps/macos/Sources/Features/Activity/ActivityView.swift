import AppKit
import SwiftUI

/// Content column: the agent activity this project has produced, newest first.
struct ActivitySessionList: View {
    @ObservedObject var model: ActivityModel

    var body: some View {
        Group {
            if self.model.sessions.isEmpty, let error = model.errorMessage {
                ContentUnavailableView {
                    Label("Activity Unavailable", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(error)
                } actions: {
                    Button("Try Again") { Task { await self.model.load() } }
                }
            } else if self.model.sessions.isEmpty && (self.model.isLoading || !self.model.hasLoaded) {
                ContentLoadingView(title: String(localized: "Loading Activity…"))
            } else if self.model.sessions.isEmpty {
                ContentUnavailableView(
                    self.model.selectedProjectId == nil ? "No Activity Yet" : "No Activity for This Project",
                    systemImage: "bubble.left.and.bubble.right",
                    description: Text(
                        self.model.selectedProjectId == nil
                            ? "Agent activity from bound projects appears here with user messages and recalled memory."
                            : "Try another project or choose All Projects."
                    )
                )
            } else {
                // Native navigation may clear the List's selection while its rows
                // are offscreen. Only an explicit row selection changes the session.
                List(selection: Binding(
                    get: { self.model.selectedSessionId },
                    set: { if let id = $0 { self.model.selectedSessionId = id } }
                )) {
                    ForEach(self.model.sessions) { session in
                        ActivitySessionRow(session: session)
                            .tag(session.id)
                    }
                    if model.pageError == nil, let cursor = model.nextCursor {
                        ProgressView()
                            .controlSize(.small)
                            .frame(maxWidth: .infinity)
                            .accessibilityLabel("More activity")
                            .task(id: cursor) { await self.model.loadMoreSessions() }
                    }
                }
                .listStyle(.inset)
                .scrollContentBackground(.hidden)
            }
        }
        .background(Color(nsColor: .controlBackgroundColor))
        .pageFeedback(model.sessions.isEmpty ? nil : model.errorMessage) { Task { await model.load() } }
        .pageFeedback(model.pageError) { Task { await model.loadMoreSessions() } }
    }
}

private struct ActivitySessionRow: View {
    let session: RecallSessionSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(self.session.activityDisplayTitle)
                .fontWeight(.medium)
                .lineLimit(1)
            HStack(spacing: 6) {
                Text(self.session.host.activityTitle)
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
struct ActivitySessionDetail: View {
    @ObservedObject var model: ActivityModel

    var body: some View {
        Group {
            Group {
                if let session = model.selectedSession {
                    if session.tasks.isEmpty {
                        ContentUnavailableView(
                            "No User Messages",
                            systemImage: "text.bubble",
                            description: Text("No user messages were found in this activity.")
                        )
                    } else {
                        self.taskList(session)
                            .navigationTitle("Activity")
                    }
                } else if let summary = model.selectedSummary {
                    VStack(alignment: .leading, spacing: 16) {
                        Text(summary.activityDisplayTitle).font(.title2.weight(.semibold))
                        if let error = model.detailError {
                            Text(error).foregroundStyle(.secondary)
                            Button("Try Again") { Task { await self.model.loadSelectedSession() } }
                            Button("Refresh Activity") { Task { await self.model.load() } }
                            Spacer()
                        } else {
                            ContentLoadingView(title: String(localized: "Activity details"))
                        }
                    }
                    .padding(24)
                } else {
                    ContentUnavailableView(
                        "Select an Activity",
                        systemImage: "sidebar.left",
                        description: Text("Choose an item to see its messages and recalled memory.")
                    )
                }
            }
        }
        .pageFeedback(model.selectedSession == nil ? nil : model.detailError) { Task { await model.retryDetail() } }
        .id(model.selectedSessionId)
        .task(id: model.selectedSummary?.sessionToken) { await self.model.loadSelectedSession() }
    }

    private func taskList(_ session: RecallSession) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                ActivitySessionHeader(session: session, totalTasks: self.model.totalTasks)

                ForEach(session.tasks) { task in
                    Divider()
                    ActivityTaskSection(
                        task: task,
                        session: session,
                        model: self.model
                    )
                }
                if model.detailError == nil, let offset = model.nextTaskOffset {
                    ProgressView()
                        .controlSize(.small)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical)
                        .accessibilityLabel("More messages")
                        .task(id: offset) { await self.model.loadMoreTasks() }
                } else if self.model.isLoadingDetail {
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

private struct ActivitySessionHeader: View {
    let session: RecallSession

    let totalTasks: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(self.session.activityDisplayTitle)
                .font(.title2.weight(.semibold))
                .lineLimit(3)
                .textSelection(.enabled)

            HStack(spacing: 6) {
                Text(self.session.host.activityTitle)
                if let createdAt = session.createdAt {
                    Text("·")
                    Text(Self.date(createdAt))
                }
            }
            .foregroundStyle(.secondary)

            Label(
                "\(self.totalTasks) user messages",
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

private struct ActivityTaskSection: View {
    let task: RecallTask
    let session: RecallSession
    let model: ActivityModel

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Label("User message", systemImage: "person.crop.circle")
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(.secondary)
                    Spacer()
                    if let time = task.time {
                        Text(Self.date(time))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Text(self.task.text)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            if self.task.activations.isEmpty {
                Label(
                    "The agent did not ask Clumsies for memory while handling this message.",
                    systemImage: "brain"
                )
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(self.task.activations) { activation in
                    ActivityActivationRow(
                        activation: activation,
                        workspaceRoot: self.session.workspaceRoot,
                        model: self.model,
                        onOpenRetrieval: {
                            self.model.openRetrieval(
                                session: self.session,
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

private struct ActivityActivationRow: View {
    let activation: RecallActivation
    let workspaceRoot: String
    let model: ActivityModel
    let onOpenRetrieval: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Label("Memory search", systemImage: "sparkle.magnifyingglass")
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.secondary)
                Spacer()
                if let status = Self.visibleStatusTitle(activation.runStatus) {
                    Text(status)
                        .font(.caption)
                        .foregroundStyle(["failed", "error"].contains(activation.runStatus?.lowercased() ?? "") ? Color.orange : Color.secondary)
                }
            }

            Text(self.activation.query)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)

            HStack(spacing: 10) {
                if !self.activation.fragments.isEmpty {
                    Text("\(self.activation.fragments.count) selected chunks")
                }
                if let totalUs = activation.totalUs {
                    Text(Duration.microseconds(Int64(clamping: totalUs)).formatted(.units(allowed: [.seconds, .milliseconds], width: .abbreviated)))
                }
                Spacer()
                if self.activation.runId != nil {
                    Button("Retrieval Process", systemImage: "chevron.right", action: self.onOpenRetrieval)
                        .buttonStyle(.borderless)
                        .accessibilityLabel("View retrieval process for \(self.activation.query)")
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

            if self.activation.fragments.isEmpty {
                Label(
                    "No matching memory chunks were returned.",
                    systemImage: "doc.text.magnifyingglass"
                )
                .font(.caption)
                .foregroundStyle(.secondary)
            } else {
                VStack(spacing: 8) {
                    ForEach(self.activation.fragments) { fragment in
                        ActivityFragmentRow(
                            fragment: fragment,
                            workspaceRoot: self.workspaceRoot,
                            runId: self.activation.runId,
                            model: self.model
                        )
                        .id(fragment.unitKey + ":" + (self.activation.runId ?? ""))
                    }
                }
            }
        }
    }

    private static func visibleStatusTitle(_ status: String?) -> String? {
        switch status?.lowercased() {
        case nil, "succeeded", "success", "completed": nil
        case "running": String(localized: "Searching")
        case "failed", "error": String(localized: "Failed")
        case let status?: status.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }
}

struct ActivityFragmentRow: View {
    let fragment: RecallFragment
    let workspaceRoot: String
    let runId: String?
    let model: ActivityModel
    @StateObject private var content = ActivityFragmentModel()
    @State private var isExpanded = false

    private var displayedFragment: RecallFragment { content.fullFragment ?? fragment }

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 8) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(self.fragment.displayTitle)
                        .font(.subheadline.weight(.medium))
                        .lineLimit(1)
                        .help(self.fragment.displayTitle)
                    Text(self.fragment.locationTitle)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(self.fragment.locationTitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    HStack(spacing: 8) {
                        Text(self.fragment.scopeTitle)
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

                if !self.displayedFragment.content.isEmpty {
                    Text(verbatim: self.displayedFragment.content)
                        .font(.callout)
                        .lineLimit(self.isExpanded ? nil : 3)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    Text(self.displayedFragment.emptyContentExplanation)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }

                Button(self.isExpanded ? "Collapse source" : "Show source") {
                    self.isExpanded.toggle()
                }
                .buttonStyle(.borderless)
                .font(.caption)

                if self.isExpanded && self.content.isLoading {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Loading the recorded chunk…")
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                } else if self.isExpanded && self.displayedFragment.truncated {
                    Text("Only the recorded preview is available for this chunk.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(8)
        }
        .pageFeedback(isExpanded && content.loadFailed
            ? String(localized: "The full retrieval record is unavailable. Showing the recorded preview.") : nil) {
            Task { await loadFullFragment() }
        }
        .task(id: self.isExpanded) {
            if self.isExpanded { await self.loadFullFragment() }
        }
    }

    private func loadFullFragment() async {
        await content.load(fragment: fragment, runId: runId) {
            guard let runId else { throw CancellationError() }
            return try await model.loadFragment(workspaceRoot: workspaceRoot, runId: runId, unitKey: fragment.unitKey)
        }
    }
}

struct ActivityRetrievalDetail: View {
    let selection: ActivityRetrievalSelection
    @StateObject private var retrieval: RetrievalDiagnosticsModel

    init(selection: ActivityRetrievalSelection, daemon: DaemonXPCClient) {
        self.selection = selection
        _retrieval = StateObject(wrappedValue: RetrievalDiagnosticsModel(daemon: daemon))
    }

    var body: some View {
        RetrievalRunDetailView(model: self.retrieval)
        .background(Color(nsColor: .textBackgroundColor))
        .navigationTitle("Retrieval Process")
        .task(id: selection.runId) { await self.retrieval.select(runId: self.selection.runId) }
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
        case "add": String(localized: "Sent to agent")
        case "replace": String(localized: "Updated for agent")
        case "reuse": String(localized: "Already available")
        default: nil
        }
    }

    fileprivate var emptyContentExplanation: String {
        action?.lowercased() == "reuse"
            ? String(localized: "The agent already had this unchanged memory chunk, so its text was not sent again.")
            : String(localized: "The full text was not preserved in this activity record.")
    }

    fileprivate var scopeTitle: String {
        switch scope {
        case .org?: String(localized: "Remote memory")
        case .project?: String(localized: "Project memory")
        case nil: String(localized: "Memory")
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
            ?? String(localized: "Agent activity")
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
        case .manual: String(localized: "Manual")
        case .zed: "Zed"
        case .unknown: String(localized: "Unknown")
        }
    }
}
