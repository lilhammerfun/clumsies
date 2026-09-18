import Combine
import Foundation
import SwiftUI

enum SettingsPane: String, CaseIterable, Identifiable {
    case general, agent, organization, advanced

    var id: Self { self }
    static let defaultsKey = "ClumsiesSettingsPane"

    var title: String {
        switch self {
        case .general: "General"
        case .agent: "Agents"
        case .organization: "Organization"
        case .advanced: "Support"
        }
    }

    var systemImage: String {
        switch self {
        case .general: "gearshape.fill"
        case .agent: "puzzlepiece.extension.fill"
        case .organization: "building.2.fill"
        case .advanced: "questionmark.circle.fill"
        }
    }

    var color: Color {
        switch self {
        case .general: .gray
        case .agent: .purple
        case .organization: .blue
        case .advanced: .gray
        }
    }

    static func restored(from defaults: UserDefaults = .standard) -> Self {
        defaults.string(forKey: defaultsKey).flatMap(Self.init(rawValue:)) ?? .general
    }

    func persist(in defaults: UserDefaults = .standard) {
        defaults.set(rawValue, forKey: Self.defaultsKey)
    }
}

enum SettingsDestination: Hashable, Identifiable {
    case pane(SettingsPane)
    case organization(AdministrationSection)

    var id: Self { self }
    var pane: SettingsPane {
        switch self {
        case .pane(let pane): pane
        case .organization: .organization
        }
    }
    var title: String {
        switch self {
        case .pane(let pane): pane.title
        case .organization(.organization): "Organization Details"
        case .organization(.audit): "Audit Log"
        case .organization(let section): section.title
        }
    }
    var subtitle: String {
        switch self {
        case .pane(.general): "Version and software updates"
        case .pane(.agent): "Agent integrations for this Mac"
        case .pane(.organization): "Organization name, members, and sign-in"
        case .pane(.advanced): "Troubleshooting logs"
        case .organization(.organization): "Organization name"
        case .organization(.members): "Invitations, roles, and membership"
        case .organization(.projects): "Projects and project members"
        case .organization(.access): "Single sign-on and allowed email domains"
        case .organization(.audit): "Organization activity and changes"
        }
    }
    var symbol: String {
        switch self {
        case .pane(let pane): pane.systemImage
        case .organization(let section): section.symbol
        }
    }

    private var keywords: String {
        switch self {
        case .pane(.general): "about automatic download software update version"
        case .pane(.agent): "plugin repair mcp integration repository"
        case .pane(.organization): "team administration name rename"
        case .pane(.advanced): "diagnostics logs help troubleshooting"
        case .organization(.organization): "name rename"
        case .organization(.members): "add roles disable users"
        case .organization(.projects): "create delete project members"
        case .organization(.access): "SSO login sign-in email domains identity provider"
        case .organization(.audit): "events activity history"
        }
    }

    static func search(_ query: String, canAdminister: Bool) -> [Self] {
        let destinations = SettingsPane.allCases
            .filter { $0 != .organization || canAdminister }.map(Self.pane)
            + (canAdminister ? AdministrationSection.allCases.filter { $0 != .organization }.map(Self.organization) : [])
        let words = query.split(whereSeparator: { $0.isWhitespace }).map(String.init)
        return destinations.filter { destination in
            let text = "\(destination.title) \(destination.subtitle) \(destination.pane.title) \(destination.keywords)"
            return words.allSatisfy { text.localizedStandardContains($0) }
        }
    }
}

@MainActor
final class SettingsNavigation: ObservableObject {
    @Published private(set) var destination: SettingsDestination
    @Published var pendingDestination: SettingsDestination?
    @Published var hasUnsavedChanges = false
    @Published var isSaving = false
    @Published var query = ""
    @Published private(set) var contentGeneration = UUID()
    private let defaults: UserDefaults
    private var history: [SettingsDestination]
    private var historyIndex = 0
    private var pendingHistoryIndex: Int?
    var canGoBack: Bool { historyIndex > 0 }
    var canGoForward: Bool { historyIndex + 1 < history.count }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let initial = SettingsDestination.pane(SettingsPane.restored(from: defaults))
        destination = initial
        history = [initial]
    }

    func navigate(to next: SettingsDestination) { request(next, historyIndex: nil) }

    func goBack() {
        guard canGoBack else { return }
        request(history[historyIndex - 1], historyIndex: historyIndex - 1)
    }

    func goForward() {
        guard canGoForward else { return }
        request(history[historyIndex + 1], historyIndex: historyIndex + 1)
    }

    private func request(_ next: SettingsDestination, historyIndex: Int?) {
        guard !(isSaving && hasUnsavedChanges), next != destination else { return }
        if hasUnsavedChanges {
            pendingHistoryIndex = historyIndex
            pendingDestination = next
        } else {
            apply(next, historyIndex: historyIndex)
        }
    }

    func discardAndNavigate() {
        guard !isSaving, let next = pendingDestination else { return }
        hasUnsavedChanges = false
        pendingDestination = nil
        apply(next, historyIndex: pendingHistoryIndex)
        pendingHistoryIndex = nil
    }

    func resetForAuthorityChange() {
        hasUnsavedChanges = false
        isSaving = false
        pendingDestination = nil
        query = ""
        contentGeneration = UUID()
        pendingHistoryIndex = nil
        let next: SettingsDestination = destination.pane == .organization ? .pane(.general) : destination
        history = [next]
        historyIndex = 0
        destination = next
        next.pane.persist(in: defaults)
    }

    private func apply(_ next: SettingsDestination, historyIndex nextIndex: Int?) {
        if let nextIndex {
            historyIndex = nextIndex
        } else {
            history = Array(history.prefix(historyIndex + 1)) + [next]
            historyIndex = history.count - 1
        }
        destination = next
        next.pane.persist(in: defaults)
    }
}
