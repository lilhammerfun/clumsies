import Combine
import Foundation
import SwiftUI

/// Owns only refresh timing and tasks; feature models still own data and errors.
@MainActor
final class WorkspaceRefreshScheduler: ObservableObject {
    enum Domain: String, CaseIterable {
        case sync, memory, reviews, inbox, dashboard, reviewDetail, activity
    }

    enum Result: Equatable { case updated, retained, deferred }

    struct Status {
        var isRefreshing = false
        var lastSuccess: Date?
        var isStale = false
        var requestID: String?
    }

    private struct Job {
        let id = UUID()
        let action: @MainActor () async -> Result
        var task: Task<Void, Never>?
        var lastAttempt: Date?
        var invalidated = true
    }

    @Published private(set) var statuses: [Domain: Status] = [:]
    private var jobs: [Domain: Job] = [:]
    private let now: () -> Date
    var isForeground = true
    @Published var visible: Domain = .memory

    init(now: @escaping () -> Date = Date.init) { self.now = now }

    @discardableResult
    func register(_ domain: Domain, action: @escaping @MainActor () async -> Result) -> UUID {
        jobs[domain]?.task?.cancel()
        let job = Job(action: action)
        jobs[domain] = job
        statuses[domain] = Status()
        return job.id
    }

    func unregister(_ domain: Domain, id: UUID) {
        guard jobs[domain]?.id == id else { return }
        jobs.removeValue(forKey: domain)?.task?.cancel()
        statuses[domain] = nil
    }

    // ponytail: poll existing loaders; add a daemon revision probe if full-list scans become costly.
    func interval(for domain: Domain) -> TimeInterval {
        if !isForeground { return 60 }
        if domain == .sync { return 2 }
        if domain == .dashboard { return 60 }
        return domain == visible || (visible == .reviewDetail && domain == .reviews) ? 5 : 30
    }

    func show(_ domain: Domain, isForeground foreground: Bool) {
        let reentered = foreground && (!isForeground || visible != domain)
        isForeground = foreground
        if visible != domain { visible = domain }
        if reentered, statuses[domain]?.lastSuccess.map({ now().timeIntervalSince($0) < interval(for: domain) }) != true {
            invalidate([domain])
        }
    }

    private func isVisible(_ domain: Domain) -> Bool {
        domain == visible || (visible == .reviewDetail && domain == .reviews)
    }

    func tick() {
        for domain in Domain.allCases {
            guard let job = jobs[domain], job.task == nil else { continue }
            if (job.invalidated && isForeground && isVisible(domain)) || job.lastAttempt.map({ now().timeIntervalSince($0) >= interval(for: domain) }) != false {
                request(domain)
            }
        }
    }

    /// A burst during a read causes one trailing read, not concurrent duplicates.
    func invalidate(_ domains: [Domain], immediately: Bool = true) {
        for domain in domains {
            jobs[domain]?.invalidated = true
            if immediately && isForeground && isVisible(domain) { request(domain) }
        }
    }

    @discardableResult
    func request(_ domain: Domain) -> Task<Void, Never>? {
        guard var job = jobs[domain] else { return nil }
        if let task = job.task { return task }
        job.invalidated = false
        job.lastAttempt = now()
        let id = job.id
        let action = job.action
        statuses[domain, default: Status()].isRefreshing = true
        job.task = Task { [weak self] in
            guard !Task.isCancelled else { return }
            let started = ContinuousClock.now
            let requestID = "req_" + UUID().uuidString.lowercased()
            ClientDiagnostics.record("workspace_refresh_started", ["domain": domain.rawValue, "request_id": requestID])
            let result = await ClientDiagnostics.$requestID.withValue(requestID) { await action() }
            guard let self, !Task.isCancelled, self.jobs[domain]?.id == id else { return }
            self.statuses[domain, default: Status()].requestID = requestID
            self.jobs[domain]?.task = nil
            self.statuses[domain, default: Status()].isRefreshing = false
            switch result {
            case .updated:
                self.statuses[domain, default: Status()].lastSuccess = self.now()
                self.statuses[domain, default: Status()].isStale = false
            case .retained:
                self.statuses[domain, default: Status()].isStale = true
            case .deferred: break
            }
            let elapsed = started.duration(to: .now).components
            ClientDiagnostics.record("workspace_refresh_completed", [
                "domain": domain.rawValue,
                "request_id": requestID,
                "result": String(describing: result),
                "elapsed_ms": String(elapsed.seconds * 1_000 + elapsed.attoseconds / 1_000_000_000_000_000)
            ])
            if self.jobs[domain]?.invalidated == true, self.isForeground, self.isVisible(domain) { self.request(domain) }
        }
        jobs[domain] = job
        return job.task
    }

    /// Drop timing and in-flight reads on context changes; retain the registered loaders.
    func reset() {
        cancel()
        for domain in Array(jobs.keys) {
            jobs[domain]?.lastAttempt = nil
            jobs[domain]?.invalidated = true
            statuses[domain] = Status()
        }
    }

    func cancel() {
        for domain in Array(jobs.keys) {
            jobs[domain]?.task?.cancel()
            jobs[domain]?.task = nil
            statuses[domain]?.isRefreshing = false
        }
    }
}

struct WorkspaceRefreshStatusView: View {
    @ObservedObject var scheduler: WorkspaceRefreshScheduler

    var body: some View {
        let domain = scheduler.visible
        let status = scheduler.statuses[domain] ?? .init()
        Group {
            if status.isStale {
                HStack(spacing: 8) {
                    Label("Showing previous data. Updates will resume automatically.", systemImage: "clock.badge.exclamationmark")
                    Spacer()
                    Button("Retry") { scheduler.request(domain) }
                        .disabled(status.isRefreshing)
                        .accessibilityIdentifier("workspace-refresh")
                }
                .font(.caption).foregroundStyle(.secondary)
                .padding(.horizontal, 12).padding(.vertical, 6)
                .background(.bar)
            }
        }
        .onChange(of: status.requestID) { _, requestID in
            if let requestID {
                ClientDiagnostics.record("workspace_refresh_presented", ["domain": domain.rawValue, "request_id": requestID])
            }
        }
    }
}
