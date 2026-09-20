import Foundation

struct DashboardResource: Codable, Identifiable, Sendable {
    let id: String
    let title: String
    let path: String
}

struct DashboardMemoryStatistics: Codable, Sendable {
    let generatedAt: TimeInterval
    let dayBounds: [TimeInterval]
    let recencyStarts: [TimeInterval]
    let projectIds: [String]
    let resources: [DashboardResource]
    let memoryCount: Int
    let addedCount: Int
    let updatedCount: Int
    let deletedCount: Int
    let days: [InventoryDay]
    let changeBuckets: [DashboardChangeBucket]
    let openDrafts: Int
    let submittedDrafts: Int

    struct InventoryDay: Codable, Sendable {
        let date: TimeInterval
        let memoryCount: Int?
    }
}

struct DashboardRetrievalRequest: Encodable, Sendable {
    let projectIds: [String]
    let resources: [DashboardResource]
    let dayBounds: [TimeInterval]
    let recencyStarts: [TimeInterval]
    let generatedAt: TimeInterval
}

struct DashboardRetrievalStatistics: Codable, Sendable {
    let retrievals: Int
    let recalledCount: Int
    let coverage: Double
    let days: [RetrievalDay]
    let directories: [DashboardBar]
    let topResources: [DashboardBar]
    let recency: [DashboardBar]
    let historyStart: TimeInterval?
    let retentionPerProject: Int

    struct RetrievalDay: Codable, Sendable {
        let date: TimeInterval
        let returned: Int
        let empty: Int
        let failed: Int
    }
}

struct DashboardDay: Identifiable, Sendable {
    var id: Date { date }
    let date: Date
    let memoryCount: Int?
    var returned = 0
    var empty = 0
    var failed = 0
    var retrievalObserved = false
}

struct DashboardSnapshot: Codable, Sendable {
    let projectId: String?
    let projectName: String
    let period: Int
    let memory: DashboardMemoryStatistics
    let retrieval: DashboardRetrievalStatistics
    var notice: String? = nil
    var projectID: String? { projectId }
    var generatedAt: Date { Date(timeIntervalSince1970: memory.generatedAt) }
    var openDrafts: Int { memory.openDrafts }
    var submittedDrafts: Int { memory.submittedDrafts }
}

struct DashboardBar: Codable, Identifiable, Sendable {
    let id: String
    let label: String
    let value: Int
    var total: Int? = nil
}

struct DashboardChangeBucket: Codable, Identifiable, Sendable {
    var id: String { "\(timestamp):\(kind.rawValue)" }
    var date: Date { Date(timeIntervalSince1970: timestamp) }
    let timestamp: TimeInterval
    let kind: Kind
    let count: Int
    enum CodingKeys: String, CodingKey { case timestamp = "date", kind, count }
    enum Kind: String, Codable, Sendable {
        case added, updated, deleted
        var title: String {
            switch self { case .added: "Added"; case .updated: "Updated"; case .deleted: "Deleted" }
        }
    }
}

/// Presentation adapter only. Counts, windows, rankings and buckets come from their producers.
struct DashboardSummary {
    let snapshot: DashboardSnapshot
    var resources: [DashboardResource] { snapshot.memory.resources }
    var memoryCount: Int { snapshot.memory.memoryCount }
    var chartStart: Date { Date(timeIntervalSince1970: snapshot.memory.dayBounds.first ?? snapshot.memory.generatedAt) }
    var chartEnd: Date { Date(timeIntervalSince1970: snapshot.memory.dayBounds.last ?? snapshot.memory.generatedAt + 1) }
    var retrievals: Int { snapshot.retrieval.retrievals }
    var recalledCount: Int { snapshot.retrieval.recalledCount }
    var coverage: Double { snapshot.retrieval.coverage }
    var directories: [DashboardBar] { snapshot.retrieval.directories }
    var topResources: [DashboardBar] { snapshot.retrieval.topResources }
    var recency: [DashboardBar] { snapshot.retrieval.recency }
    var changeBuckets: [DashboardChangeBucket] { snapshot.memory.changeBuckets }
    func changedCount(_ kind: DashboardChangeBucket.Kind) -> Int {
        switch kind {
        case .added: snapshot.memory.addedCount
        case .updated: snapshot.memory.updatedCount
        case .deleted: snapshot.memory.deletedCount
        }
    }
    var days: [DashboardDay] {
        snapshot.memory.days.map { inventory in
            let retrieval = snapshot.retrieval.days.first { $0.date == inventory.date }
            return DashboardDay(date: Date(timeIntervalSince1970: inventory.date), memoryCount: inventory.memoryCount,
                returned: retrieval?.returned ?? 0, empty: retrieval?.empty ?? 0, failed: retrieval?.failed ?? 0,
                retrievalObserved: retrieval != nil)
        }
    }
}
