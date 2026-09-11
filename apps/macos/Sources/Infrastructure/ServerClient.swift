import Foundation

enum ServerClientError: LocalizedError, Sendable {
    case invalidPath
    case forbidden(String)
    case response(status: Int, message: String)
    case invalidResponse(String)

    var errorDescription: String? {
        switch self {
        case .invalidPath:
            return "The Server request path is invalid."
        case .forbidden(let message):
            return message
        case .response(let status, let message):
            return "Server request failed (\(status)): \(message)"
        case .invalidResponse(let message):
            return "The Server returned invalid data: \(message)"
        }
    }
}

struct ServerClient: Sendable {
    let daemon: DaemonXPCClient
    private let dataSourceTracker = ServerDataSourceTracker()
    private let requestLimiter = ServerRequestLimiter(limit: 12)

    var dataSource: String { dataSourceTracker.value }

    func resetDataSource() {
        dataSourceTracker.reset()
    }

    func get<Response: Decodable & Sendable>(
        _ path: String,
        query: [URLQueryItem] = [],
        headers: [String: String] = [:]
    ) async throws -> Response {
        try await request(method: "GET", path: path, query: query, headers: headers, body: nil)
    }

    func getWithMetadata<Response: Decodable & Sendable>(
        _ path: String,
        query: [URLQueryItem] = [],
        headers: [String: String] = [:]
    ) async throws -> (value: Response, response: DaemonServerResponse) {
        let response = try await raw(method: "GET", path: path, query: query, headers: headers)
        let requestPath = (try? buildPath(path, query: query)) ?? path
        let value: Response = try decode(response, method: "GET", path: requestPath)
        return (value, response)
    }

    func send<Response: Decodable & Sendable, Body: Encodable & Sendable>(
        method: String,
        path: String,
        query: [URLQueryItem] = [],
        headers: [String: String] = [:],
        body: Body
    ) async throws -> Response {
        let data: Data
        do {
            data = try JSONCoding.encoder().encode(body)
        } catch {
            ClientDiagnostics.record("request_encode_failed", ["request_id": ClientDiagnostics.requestID ?? "req_" + UUID().uuidString.lowercased(),
                "route": ClientDiagnostics.route(path), "method": method, "kind": "encode"])
            throw error
        }
        guard let bodyString = String(data: data, encoding: .utf8) else {
            throw ServerClientError.invalidResponse("Could not encode the request body.")
        }
        return try await request(
            method: method,
            path: path,
            query: query,
            headers: headers.merging(["content-type": "application/json"], uniquingKeysWith: { current, _ in current }),
            body: bodyString
        )
    }

    func raw(
        method: String,
        path: String,
        query: [URLQueryItem] = [],
        headers: [String: String] = [:],
        body: String? = nil
    ) async throws -> DaemonServerResponse {
        try await ClientDiagnostics.operation(layer: "server", method: method) {
            try await performRaw(method: method, path: path, query: query, headers: headers, body: body)
        }
    }

    private func performRaw(method: String, path: String, query: [URLQueryItem], headers: [String: String], body: String?) async throws -> DaemonServerResponse {
        let requestPath = try buildPath(path, query: query)
        let dataSourceGeneration = dataSourceTracker.generation
        let response = try await requestLimiter.run {
            try Task.checkCancellation()
            return try await daemon.serverRequest(
                .init(method: method, path: requestPath, headers: headers, body: body)
            )
        }
        if response.isStaleCache, !Task.isCancelled {
            dataSourceTracker.markStale(generation: dataSourceGeneration)
        }
        ClientDiagnostics.record((200..<300).contains(response.status) ? "server_completed" : "server_failed", [
            "request_id": ClientDiagnostics.requestID ?? "", "method": method, "route": ClientDiagnostics.route(path),
            "status": String(response.status), "source": response.isStaleCache ? "cache" : "server",
            "server_request_id": ClientDiagnostics.identifier(response.headers["x-request-id"] ?? "")
        ])
        return DaemonServerResponse(status: response.status,
            headers: response.headers.merging(["x-clumsies-request-id": ClientDiagnostics.requestID ?? ""], uniquingKeysWith: { _, current in current }),
            body: response.body)
    }

    private func request<Response: Decodable & Sendable>(
        method: String,
        path: String,
        query: [URLQueryItem],
        headers: [String: String],
        body: String?
    ) async throws -> Response {
        let response = try await raw(method: method, path: path, query: query, headers: headers, body: body)
        let requestPath = (try? buildPath(path, query: query)) ?? path
        return try decode(response, method: method, path: requestPath)
    }

    func decode<Response: Decodable & Sendable>(
        _ response: DaemonServerResponse,
        method: String,
        path: String
    ) throws -> Response {
        guard (200..<300).contains(response.status) else {
            let message = Self.errorMessage(from: response.body)
            let requestID = response.headers["x-request-id"].map { " (request \(ClientDiagnostics.identifier($0)))" } ?? ""
            throw ServerClientError.response(status: response.status, message: message + requestID)
        }
        guard let data = response.body.data(using: .utf8) else {
            throw ServerClientError.invalidResponse("Response body is not UTF-8.")
        }
        do {
            return try JSONCoding.decoder().decode(Response.self, from: data)
        } catch {
            let message = Self.decodingFailureMessage(
                error,
                method: method,
                path: path,
                responseType: Response.self
            )
            ClientDiagnostics.record("response_decode_failed", ["method": method, "route": ClientDiagnostics.route(path),
                "kind": "decode", "request_id": response.headers["x-clumsies-request-id"] ?? "", "server_request_id": ClientDiagnostics.identifier(response.headers["x-request-id"] ?? "")])
            throw ServerClientError.invalidResponse(message)
        }
    }

    static func decodingFailureMessage(
        _ error: Error,
        method: String,
        path: String,
        responseType: Any.Type
    ) -> String {
        let detail: String
        switch error {
        case DecodingError.dataCorrupted(let context):
            detail = decodingDetail(context.debugDescription, path: context.codingPath)
        case DecodingError.keyNotFound(let key, let context):
            detail = decodingDetail("Missing key '\(key.stringValue)'.", path: context.codingPath + [key])
        case DecodingError.typeMismatch(let type, let context):
            detail = decodingDetail("Expected \(type). \(context.debugDescription)", path: context.codingPath)
        case DecodingError.valueNotFound(let type, let context):
            detail = decodingDetail("Missing \(type) value. \(context.debugDescription)", path: context.codingPath)
        default:
            detail = error.localizedDescription
        }
        return "\(method) \(path) could not decode \(responseType): \(detail)"
    }

    private static func decodingDetail(_ description: String, path: [CodingKey]) -> String {
        let field = path.reduce(into: "") { result, key in
            if let index = key.intValue {
                result += "[\(index)]"
            } else {
                if !result.isEmpty { result += "." }
                result += key.stringValue
            }
        }
        return field.isEmpty ? description : "\(field): \(description)"
    }

    private func buildPath(_ path: String, query: [URLQueryItem]) throws -> String {
        guard path.hasPrefix("/"), var components = URLComponents(string: path) else {
            throw ServerClientError.invalidPath
        }
        if !query.isEmpty {
            components.queryItems = query
        }
        guard let value = components.string else {
            throw ServerClientError.invalidPath
        }
        return value
    }

    private static func errorMessage(from body: String) -> String {
        guard let data = body.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let error = object["error"] as? [String: Any],
              let message = error["message"] as? String else {
            return body.isEmpty ? "No error details were returned." : body
        }
        return message
    }
}

extension DaemonServerResponse {
    var isStaleCache: Bool {
        headers.contains {
            $0.key.caseInsensitiveCompare("x-clumsies-cache") == .orderedSame
                && $0.value.caseInsensitiveCompare("stale") == .orderedSame
        }
    }
}

private actor ServerRequestLimiter {
    private var available: Int
    private var waiters: [CheckedContinuation<Void, Never>] = []

    init(limit: Int) {
        available = max(1, limit)
    }

    func run<Output: Sendable>(
        _ operation: @Sendable () async throws -> Output
    ) async throws -> Output {
        await acquire()
        defer { release() }
        return try await operation()
    }

    private func acquire() async {
        if available > 0 {
            available -= 1
            return
        }
        await withCheckedContinuation { continuation in
            waiters.append(continuation)
        }
    }

    private func release() {
        if waiters.isEmpty {
            available += 1
        } else {
            waiters.removeFirst().resume()
        }
    }
}

final class ServerDataSourceTracker: @unchecked Sendable {
    private let lock = NSLock()
    private var stale = false
    private var currentGeneration = UUID()

    var value: String {
        lock.withLock { stale ? "stale" : "live" }
    }

    var generation: UUID {
        lock.withLock { currentGeneration }
    }

    func markStale(generation: UUID) {
        lock.withLock {
            guard generation == currentGeneration else { return }
            stale = true
        }
    }

    func reset() {
        lock.withLock {
            currentGeneration = UUID()
            stale = false
        }
    }
}
