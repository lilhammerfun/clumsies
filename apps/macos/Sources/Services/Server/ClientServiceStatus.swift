import Combine
import Foundation

/// One connection indication per window, shared by the app's existing request boundaries.
@MainActor
final class ClientServiceStatus: ObservableObject {
    static let shared = ClientServiceStatus()
    @Published private(set) var failure: ClientFailure?
    private var requests: [String: UUID] = [:]
    private var failures: [String: ClientFailure] = [:]

    func begin(_ route: String) -> UUID {
        let token = UUID()
        requests[route] = token
        return token
    }

    func finish(_ route: String, token: UUID, failure: ClientFailure?) {
        guard requests[route] == token else { return }
        requests.removeValue(forKey: route)
        guard failure != .cancelled else { return }
        if failure == nil {
            if route.hasPrefix("daemon:") {
                failures = failures.filter { $0.value != .localService }
            } else if route.hasPrefix("server:") {
                failures = failures.filter { $0.value != .connection && $0.value != .authentication }
            }
        }
        failures[route] = failure.flatMap { $0.isServiceFailure ? $0 : nil }
        // Local success does not establish remote connectivity, and a healthy route
        // must not clear a different route's server failure.
        let priorities: [ClientFailure] = [.authentication, .localService, .connection, .timeout, .service, .rateLimited]
        self.failure = priorities.first { failures.values.contains($0) }
    }

    func dismiss() {
        failures.removeAll()
        failure = nil
    }

    func reset() {
        requests.removeAll()
        failures.removeAll()
        failure = nil
    }
}
