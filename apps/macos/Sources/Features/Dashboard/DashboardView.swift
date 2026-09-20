import Charts
import SwiftUI

struct DashboardPage: View {
    @ObservedObject var context: WorkspaceContext
    @StateObject private var model = DashboardModel()
    let onOpenMemory: (String) -> Void

    private struct Input: Hashable {
        let authority: UUID
        let projectID: String?
        let period: Int
    }

    private var input: Input {
        .init(authority: context.authorityGeneration, projectID: context.activeProjectId,
              period: model.period)
    }

    var body: some View {
        DashboardView(model: model, onRefresh: { Task { await load(input) } }, onOpenMemory: onOpenMemory)
            .task(id: input) { await load(input) }
    }

    private func load(_ input: Input) async {
        let name = context.activeProject?.name ?? context.organization?.name ?? "Organization"
        await model.load(key: "\(input.authority):\(input.projectID ?? "organization"):\(input.period)") {
            if let demo = try await DashboardModel.demoSnapshot(projectID: input.projectID, period: input.period) { return (demo, true) }
            return (try await DashboardModel.liveSnapshot(
                projectID: input.projectID, name: name, period: input.period,
                daemon: context.daemon, server: context.server
            ), false)
        }
    }
}

struct DashboardView: View {
    @ObservedObject var model: DashboardModel
    let onRefresh: () -> Void
    let onOpenMemory: (String) -> Void
    @State private var definition: DashboardMetric?
    @State private var hoveredGrowth: Date?
    @State private var hoveredRetrieval: Date?
    @Environment(\.colorScheme) private var colorScheme

    private let blue = Color(red: 0.29, green: 0.48, blue: 0.94)
    private let green = Color(red: 0.18, green: 0.64, blue: 0.55)
    private let amber = Color(red: 0.88, green: 0.64, blue: 0.36)
    private let red = Color(red: 0.80, green: 0.40, blue: 0.47)
    private var cardBackground: Color {
        colorScheme == .dark ? Color(nsColor: .controlBackgroundColor) : .white
    }

    var body: some View {
        GeometryReader { geometry in
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    if model.isDemo {
                        Label("Demo data", systemImage: "sparkles")
                            .font(.caption.weight(.medium)).foregroundStyle(blue)
                            .padding(.horizontal, 9).padding(.vertical, 5)
                            .background(blue.opacity(0.09), in: Capsule())
                    }
                    if let error = model.errorMessage {
                        HStack {
                            Label(error, systemImage: "exclamationmark.triangle")
                                .font(.callout).foregroundStyle(.orange)
                            Spacer()
                            Button("Retry", action: onRefresh)
                        }.padding(12).background(.orange.opacity(0.07), in: RoundedRectangle(cornerRadius: 8))
                    }
                    if let summary = model.summary {
                        if let notice = summary.snapshot.notice {
                            Label(notice, systemImage: "info.circle").font(.callout).foregroundStyle(.secondary)
                        }
                        metrics(summary, width: geometry.size.width)
                        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 18),
                                                 count: geometry.size.width >= 940 ? 2 : 1), spacing: 18) {
                            growth(summary)
                            retrieval(summary)
                            directories(summary)
                            topMemories(summary)
                            maintenance(summary)
                            recency(summary)
                        }
                    } else if model.isLoading {
                        ProgressView("Loading dashboard…").frame(maxWidth: .infinity, minHeight: 400)
                    } else {
                        ContentUnavailableView("Dashboard unavailable", systemImage: "chart.bar.xaxis",
                            description: Text("Refresh to load this project's statistics."))
                    }
                }.padding(28).frame(maxWidth: 1600)
                    .frame(maxWidth: .infinity)
            }
        }
        .background(Color(nsColor: .windowBackgroundColor).opacity(0.75))
        .toolbar {
            if #available(macOS 26.0, *) {
                ToolbarSpacer(.flexible)
            }
            ToolbarItemGroup(placement: .trailingPinned) {
                Picker("Period", selection: $model.period) {
                    Text("7 days").tag(7)
                    Text("30 days").tag(30)
                    Text("90 days").tag(90)
                }
                .pickerStyle(.segmented)
                .help("Dashboard period")
                .accessibilityLabel("Dashboard period")
            }
            if #available(macOS 26.0, *) {
                ToolbarSpacer(.fixed, placement: .automatic)
            }
            ToolbarItemGroup(placement: .trailingPinned) {
                Button(action: onRefresh) { Image(systemName: "arrow.clockwise") }
                    .help(model.snapshot.map {
                        "Refresh dashboard · Updated \($0.generatedAt.formatted(date: .abbreviated, time: .shortened))"
                    } ?? "Refresh dashboard")
                    .accessibilityLabel("Refresh dashboard")
                    .disabled(model.isLoading)
            }
        }
        .sheet(item: $definition) { metric in
            VStack(alignment: .leading, spacing: 16) {
                Label(metric.title, systemImage: "chart.bar.doc.horizontal").font(.title3.weight(.semibold))
                Text(metric.explanation).font(.body).foregroundStyle(.secondary).lineSpacing(5)
                if [.retrieval, .directories, .top, .recency].contains(metric),
                   let limit = model.snapshot?.retrieval.retentionPerProject {
                    Text("Based on retrieval history retained on this Mac, up to \(limit) requests per project. Activity on other devices is not included.")
                        .font(.callout).foregroundStyle(.secondary).lineSpacing(5)
                }
                HStack { Spacer(); Button("Done") { definition = nil }.keyboardShortcut(.defaultAction) }
            }.padding(28).frame(width: 480)
        }
    }

    private func metrics(_ summary: DashboardSummary, width: CGFloat) -> some View {
        let snapshot = summary.snapshot
        let coverage = summary.resources.isEmpty ? "No memory yet" : "\(Int((summary.coverage * 100).rounded()))% of current memory"
        return LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 12), count: width >= 1100 ? 6 : 3), spacing: 12) {
            metric("Published memory", value: summary.memoryCount, detail: "Current total", symbol: "brain")
            metric("New memories", value: summary.changedCount(.added), detail: "Last \(model.period) days", symbol: "doc.badge.plus")
            metric("Updated memories", value: summary.changedCount(.updated), detail: "Last \(model.period) days", symbol: "square.and.pencil")
            metric("Retrievals", value: summary.retrievals, detail: "On this Mac", symbol: "magnifyingglass")
            metric("Memories retrieved", value: summary.recalledCount, detail: coverage, symbol: "text.magnifyingglass")
            metric("Synced drafts", value: snapshot.openDrafts + snapshot.submittedDrafts,
                   detail: "\(snapshot.openDrafts) open · \(snapshot.submittedDrafts) in review", symbol: "doc.text")
        }
    }

    private func metric(_ title: String, value: Int?, detail: String, symbol: String) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text(title).font(.system(size: 11, weight: .medium)).lineLimit(1)
                Spacer(minLength: 2)
                Image(systemName: symbol).font(.system(size: 11)).foregroundStyle(blue.opacity(0.8))
            }.foregroundStyle(.secondary)
            Text(value.map { $0.formatted() } ?? "—")
                .font(.system(size: 30, weight: .semibold, design: .rounded)).monospacedDigit()
            Text(value == nil ? "History not recorded" : detail)
                .font(.system(size: 10)).foregroundStyle(.secondary).lineLimit(1)
        }.frame(maxWidth: .infinity, alignment: .leading).padding(17)
            .background(cardBackground, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.primary.opacity(0.055)))
            .accessibilityElement(children: .combine)
    }

    private func panel<Content: View>(_ metric: DashboardMetric, context: String? = nil,
                                      @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                VStack(alignment: .leading, spacing: 6) {
                    Text(metric.title).font(.system(size: 14, weight: .semibold))
                    Text(metric.subtitle).font(.system(size: 11)).foregroundStyle(.secondary)
                }
                Spacer(minLength: 10)
                if let context {
                    Text(context).font(.system(size: 10)).foregroundStyle(.secondary)
                }
            }
            content().frame(height: 205)
            Divider().opacity(0.55)
            HStack(spacing: 12) {
                Text(metric.footnote).font(.system(size: 10)).foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Button { definition = metric } label: {
                    Label("About", systemImage: "info.circle")
                        .font(.system(size: 10, weight: .medium))
                        .fixedSize().frame(minHeight: 22).contentShape(Rectangle())
                }.buttonStyle(.plain).foregroundStyle(blue)
                    .accessibilityLabel("About \(metric.title.lowercased())")
                    .help("How \(metric.title.lowercased()) is calculated")
            }
        }.padding(21).background(cardBackground, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.primary.opacity(0.055)))
    }

    private func growth(_ summary: DashboardSummary) -> some View {
        let points = summary.days.filter { $0.memoryCount != nil }
        return panel(.growth) {
            if points.isEmpty {
                empty("No inventory history yet", detail: "Historical counts require recorded snapshots.")
            } else {
                Chart {
                    ForEach(points) { day in
                        AreaMark(x: .value("Date", day.date), y: .value("Memories", day.memoryCount ?? 0))
                            .foregroundStyle(LinearGradient(colors: [blue.opacity(0.20), blue.opacity(0.015)], startPoint: .top, endPoint: .bottom))
                        LineMark(x: .value("Date", day.date), y: .value("Memories", day.memoryCount ?? 0))
                            .foregroundStyle(blue).lineStyle(StrokeStyle(lineWidth: 2.5))
                    }
                    if points.count == 1, let day = points.first {
                        PointMark(x: .value("Date", day.date), y: .value("Memories", day.memoryCount ?? 0))
                            .foregroundStyle(blue)
                    }
                    if let hoveredGrowth, let day = closest(hoveredGrowth, in: points) {
                        RuleMark(x: .value("Date", day.date)).foregroundStyle(.secondary.opacity(0.25))
                            .annotation(position: .top, alignment: .leading, overflowResolution: .init(x: .fit(to: .chart), y: .disabled)) {
                                chartTip(day.date, text: "\(day.memoryCount ?? 0) memories")
                            }
                    }
                }.chartYScale(domain: 0...(max(points.compactMap(\.memoryCount).max() ?? 1, 1) * 12 / 10 + 1))
                    .chartXAxis { AxisMarks(values: .automatic(desiredCount: 4)) { _ in AxisValueLabel(format: .dateTime.month(.abbreviated).day()) } }
                    .chartYAxis { AxisMarks(position: .leading, values: .automatic(desiredCount: 4)) }
                    .chartXScale(domain: summary.chartStart...summary.chartEnd)
                    .chartXSelection(value: $hoveredGrowth)
            }
        }
    }

    private func retrieval(_ summary: DashboardSummary) -> some View {
        let days = summary.days.filter { $0.retrievalObserved != false }
        return panel(.retrieval) {
            if days.isEmpty { empty("No retrieval records", detail: "Retrieval activity will appear here.") }
            else {
                Chart {
                    ForEach(days) { day in
                        BarMark(x: .value("Date", day.date, unit: .day), y: .value("Requests", day.returned))
                            .foregroundStyle(by: .value("Result", "With content"))
                        BarMark(x: .value("Date", day.date, unit: .day), y: .value("Requests", day.empty))
                            .foregroundStyle(by: .value("Result", "Empty"))
                        BarMark(x: .value("Date", day.date, unit: .day), y: .value("Requests", day.failed))
                            .foregroundStyle(by: .value("Result", "Failed"))
                    }
                    if let hoveredRetrieval,
                       let day = days.first(where: { Calendar.current.isDate($0.date, inSameDayAs: hoveredRetrieval) }) {
                        RuleMark(x: .value("Date", day.date, unit: .day)).foregroundStyle(.secondary.opacity(0.25))
                            .annotation(position: .top, alignment: .leading, overflowResolution: .init(x: .fit(to: .chart), y: .disabled)) {
                                chartTip(day.date, text: "\(day.returned) returned · \(day.empty) empty · \(day.failed) failed")
                            }
                    }
                }.chartForegroundStyleScale(["With content": blue, "Empty": amber, "Failed": red])
                    .chartLegend(position: .bottom, alignment: .leading, spacing: 10)
                    .chartXAxis { AxisMarks(values: .automatic(desiredCount: 4)) { _ in AxisValueLabel(format: .dateTime.month(.abbreviated).day()) } }
                    .chartYAxis { AxisMarks(position: .leading, values: .automatic(desiredCount: 4)) }
                    .chartXScale(domain: summary.chartStart...summary.chartEnd)
                    .chartXSelection(value: $hoveredRetrieval)
            }
        }
    }

    private func directories(_ summary: DashboardSummary) -> some View {
        let all = summary.directories
        let rows = all.count <= 6 ? all : Array(all.prefix(5)) + [DashboardBar(
            id: "other-directories", label: "Other directories",
            value: all.dropFirst(5).reduce(0) { $0 + $1.value },
            total: all.dropFirst(5).reduce(0) { $0 + ($1.total ?? 0) }
        )]
        return panel(.directories) {
            if rows.isEmpty { empty("No memory yet", detail: "Select memory for this project to see its distribution.") }
            else {
                VStack(alignment: .leading, spacing: 10) {
                    horizontalBars(rows, coverage: true)
                    HStack(spacing: 12) {
                        Label { Text("Retrieved") } icon: {
                            Circle().fill(blue).frame(width: 7, height: 7)
                        }
                        Label { Text("Total") } icon: {
                            Circle().fill(blue.opacity(0.12)).frame(width: 7, height: 7)
                        }
                    }.font(.system(size: 10)).foregroundStyle(.secondary)
                }
            }
        }
    }

    private func topMemories(_ summary: DashboardSummary) -> some View {
        panel(.top) {
            if summary.topResources.isEmpty { empty("No retrieved memories", detail: "Documents returned by retrieval will appear here.") }
            else { horizontalBars(summary.topResources, coverage: false) }
        }
    }

    private func horizontalBars(_ rows: [DashboardBar], coverage: Bool) -> some View {
        let maxValue = Double(max(rows.map { $0.total ?? $0.value }.max() ?? 1, 1))
        return VStack(spacing: 12) {
            ForEach(rows) { row in
                HStack(spacing: 12) {
                    if coverage {
                        Text(row.label).font(.system(size: 11)).frame(width: 150, alignment: .leading).lineLimit(1)
                    } else {
                        Button { onOpenMemory(row.id) } label: {
                            Text(row.label).font(.system(size: 11)).frame(width: 150, alignment: .leading).lineLimit(1)
                        }.buttonStyle(.plain).help(row.label)
                    }
                    GeometryReader { geometry in
                        ZStack(alignment: .leading) {
                            if let total = row.total {
                                RoundedRectangle(cornerRadius: 3).fill(blue.opacity(0.12))
                                    .frame(width: geometry.size.width * Double(total) / maxValue)
                            }
                            RoundedRectangle(cornerRadius: 3).fill(blue)
                                .frame(width: geometry.size.width * Double(row.value) / maxValue)
                        }
                    }.frame(height: 17)
                    Text(coverage ? "\(row.value) / \(row.total ?? 0)" : row.value.formatted())
                        .font(.system(size: 11)).monospacedDigit().frame(width: coverage ? 65 : 43, alignment: .trailing)
                }.help(coverage ? "\(row.label): \(row.value) retrieved of \(row.total ?? 0) current memories" : "\(row.label): \(row.value) retrievals")
                    .accessibilityElement(children: .combine)
            }
            Spacer(minLength: 0)
        }.padding(.top, 7)
    }

    private func maintenance(_ summary: DashboardSummary) -> some View {
        panel(.maintenance, context: model.period == 7 ? "Daily" : "7-day buckets") {
            if summary.changeBuckets.allSatisfy({ $0.count == 0 }) { empty("No changes in this period", detail: "Published memory changes will appear here.") }
            else {
                Chart(summary.changeBuckets) { bucket in
                    BarMark(x: .value("Starting", bucket.date.formatted(.dateTime.month(.abbreviated).day())),
                            y: .value("Documents", bucket.count))
                        .foregroundStyle(by: .value("Change", bucket.kind.title))
                        .position(by: .value("Change", bucket.kind.title))
                        .cornerRadius(2)
                        .accessibilityLabel("\(bucket.kind.title), \(bucket.date.formatted(date: .abbreviated, time: .omitted))")
                        .accessibilityValue("\(bucket.count) documents")
                }.chartForegroundStyleScale(["Added": green, "Updated": blue, "Deleted": amber])
                    .chartLegend(position: .bottom, alignment: .leading, spacing: 10)
                    .chartXAxis { AxisMarks { _ in AxisValueLabel().font(.system(size: 9)) } }
                    .chartYAxis { AxisMarks(position: .leading, values: .automatic(desiredCount: 4)) }
            }
        }
    }

    private func recency(_ summary: DashboardSummary) -> some View {
        panel(.recency, context: "90 days") {
            if summary.resources.isEmpty { empty("No memory yet", detail: "The distribution will appear after adding memory.") }
            else {
                Chart(summary.recency) { row in
                    BarMark(x: .value("Last retrieval", row.label), y: .value("Memories", row.value))
                        .foregroundStyle(blue.opacity([1.0, 0.65, 0.42, 0.23][Int(row.id) ?? 0]))
                        .cornerRadius(4)
                        .annotation(position: .top) { Text(row.value.formatted()).font(.system(size: 10)).monospacedDigit() }
                }.chartYScale(domain: 0...(max(summary.recency.map(\.value).max() ?? 1, 1) * 12 / 10 + 1))
                    .chartYAxis { AxisMarks(position: .leading, values: .automatic(desiredCount: 4)) }
                    .chartXAxis { AxisMarks { _ in AxisValueLabel().font(.system(size: 9)) } }
            }
        }
    }

    private func empty(_ title: String, detail: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: "chart.bar.xaxis").font(.system(size: 24)).foregroundStyle(.tertiary)
            Text(title).font(.callout.weight(.medium))
            Text(detail).font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }
    private func closest(_ date: Date, in points: [DashboardDay]) -> DashboardDay? {
        points.min { abs($0.date.timeIntervalSince(date)) < abs($1.date.timeIntervalSince(date)) }
    }
    private func chartTip(_ date: Date, text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(date.formatted(date: .abbreviated, time: .omitted)).foregroundStyle(.secondary)
            Text(text).fontWeight(.medium)
        }.font(.system(size: 10)).padding(8).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
            .allowsHitTesting(false)
    }
}

private enum DashboardMetric: String, Identifiable {
    case growth, retrieval, directories, top, maintenance, recency
    var id: String { rawValue }
    var title: String {
        switch self {
        case .growth: "Memory growth"
        case .retrieval: "Retrieval activity"
        case .directories: "Directory distribution & coverage"
        case .top: "Frequently retrieved memories"
        case .maintenance: "Memory maintenance"
        case .recency: "Last retrieval distribution"
        }
    }
    var subtitle: String {
        switch self {
        case .growth: "Published memory at the end of each day"
        case .retrieval: "Completed retrieval requests on this Mac"
        case .directories: "Published document count and retrieval coverage"
        case .top: "One count per document per retrieval"
        case .maintenance: "Distinct documents added, updated or deleted"
        case .recency: "Current documents by their most recent retrieval"
        }
    }
    var footnote: String {
        switch self {
        case .growth: "Document updates do not increase inventory"
        case .retrieval: "With content includes reused fragments"
        case .directories: "Coverage counts each document once within the selected period"
        case .top: "Click a document to open Memory"
        case .maintenance: "Draft edits are excluded"
        case .recency: "Not observed does not mean never retrieved"
        }
    }
    var explanation: String {
        switch self {
        case .growth: "The server calculates daily closing inventory from published commits. Project scope includes selected organization memories; selection changes affect its count. Draft overlays are excluded. Days before the first retained commit remain unknown."
        case .retrieval: "Completed memory.activate requests, separated into returned content, successful empty results, and failures. Reused fragments count as content. Requests still running are excluded. Retained local traces may not cover the entire period; these counts do not measure agent adoption or accuracy."
        case .directories: "Published documents are grouped by directory. When all documents share a parent directory, its subdirectories are shown. The light bar shows current inventory; the blue portion shows distinct current documents retrieved within the selected period. Unpublished drafts are excluded."
        case .top: "Current documents ranked by successful retrievals in the selected period. Multiple fragments from one document count once per request, including reused fragments. The six most frequently retrieved documents are shown."
        case .maintenance: "The server compares consecutive published snapshots, including project selection changes. Distinct document IDs added, updated or deleted are grouped by day or consecutive seven-day buckets. A document can appear in several buckets, so bucket totals may exceed the period's distinct count. Draft autosaves are excluded."
        case .recency: "Each current document appears once, according to its most recent observed retrieval within 90 days. Without complete history, a missing record is labelled Not observed. Low frequency alone is not evidence that a memory should be deleted."
        }
    }
}
