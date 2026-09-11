import Foundation
import XPC

enum DaemonXPCError: LocalizedError, Sendable {
    case invalidRequest
    case connectionFailed(detail: String? = nil)
    case requestTimedOut(timeout: TimeInterval? = nil)
    case invalidReply
    case daemon(APIErrorPayload)

    var errorDescription: String? {
        let logHint = "Review logs in \(ClumsiesIdentifiers.daemonLogDirectoryURL.path)."
        switch self {
        case .invalidRequest:
            return "Could not encode the daemon request."
        case .connectionFailed(let detail):
            if let detail, !detail.isEmpty {
                return "The local Clumsies daemon is unavailable (\(detail)). \(logHint)"
            }
            return "The local Clumsies daemon is unavailable. \(logHint)"
        case .requestTimedOut(let timeout):
            if let timeout {
                return "The local Clumsies daemon did not respond within \(String(format: "%.1f", timeout))s. \(logHint)"
            }
            return "The local Clumsies daemon did not respond in time. \(logHint)"
        case .invalidReply:
            return "The local Clumsies daemon returned an invalid response."
        case .daemon(let error):
            let request = error.requestId.map { " (request \($0))" } ?? ""
            return "\(error.code): \(error.message)\(request)"
        }
    }
}

struct DaemonXPCClient: Sendable {
    static let serviceName = ClumsiesIdentifiers.daemon
    private static let replyQueue = DispatchQueue(
        label: ClumsiesIdentifiers.xpcReplyQueue,
        qos: .userInitiated,
        attributes: .concurrent
    )

    let serviceName: String

    init(serviceName: String = Self.serviceName) {
        self.serviceName = serviceName
    }

    func health(timeout: TimeInterval = 3) async throws -> DaemonHealth {
        try await call(method: "health", payload: EmptyPayload(), timeout: timeout)
    }

    func projectConfig() async throws -> DaemonProjectConfig {
        try await call(method: "project_config", payload: EmptyPayload())
    }

    func replaceProjectConfig(_ update: DaemonProjectConfigUpdate) async throws -> DaemonProjectConfig {
        try await call(method: "replace_project_config", payload: update)
    }

    func selectProject(_ projectId: String) async throws -> DaemonProjectConfig {
        try await call(method: "select_project", payload: DaemonProjectSelection(projectId: projectId))
    }

    func projectBindings(_ projectId: String) async throws -> [DaemonProjectBinding] {
        let response: DaemonProjectBindingListResponse = try await call(
            method: "list_project_bindings",
            payload: DaemonProjectBindingListRequest(projectId: projectId)
        )
        return response.items
    }

    func replaceProjectBinding(_ request: DaemonProjectBindingReplaceRequest) async throws
        -> DaemonProjectBinding {
        try await call(method: "replace_project_binding", payload: request)
    }

    func removeProjectBinding(_ request: DaemonProjectBindingRemoveRequest) async throws
        -> DaemonProjectBindingRemoveResponse {
        try await call(method: "remove_project_binding", payload: request)
    }

    func projectAgentAdapters(_ projectId: String) async throws -> [DaemonProjectAgentAdapter] {
        let response: DaemonProjectAgentAdapterListResponse = try await call(
            method: "list_project_agent_adapters",
            payload: DaemonProjectAgentAdapterListRequest(projectId: projectId)
        )
        return response.items
    }

    func allProjectAgentAdapters() async throws -> [DaemonProjectAgentAdapter] {
        let response: DaemonProjectAgentAdapterListResponse = try await call(
            method: "list_all_project_agent_adapters",
            payload: EmptyPayload()
        )
        return response.items
    }

    func inspectLegacyAgentAdapters(
        runtimeBinaryPath: String
    ) async throws -> DaemonLegacyAgentAdapterInspectionResponse {
        try await call(
            method: "inspect_legacy_agent_adapters",
            payload: DaemonLegacyAgentAdapterInspectionRequest(
                runtimeBinaryPath: runtimeBinaryPath
            ),
            timeout: 2
        )
    }

    func inspectCodexPlugin(_ request: DaemonCodexPluginRequest) async throws
        -> DaemonCodexPluginStatus {
        try await call(method: "inspect_codex_plugin", payload: request, timeout: 40)
    }

    func reconcileCodexPlugin(_ request: DaemonCodexPluginRequest) async throws
        -> DaemonCodexPluginStatus {
        try await call(method: "reconcile_codex_plugin", payload: request, timeout: 130)
    }

    func installProjectAgentAdapter(_ request: DaemonProjectAgentAdapterInstallRequest) async throws
        -> DaemonProjectAgentAdapter {
        try await call(method: "install_project_agent_adapter", payload: request)
    }

    func removeProjectAgentAdapter(_ request: DaemonProjectAgentAdapterRemoveRequest) async throws
        -> DaemonProjectAgentAdapterRemoveResponse {
        try await call(method: "remove_project_agent_adapter", payload: request)
    }

    func projectCheckout(_ projectId: String) async throws -> DaemonProjectCheckout {
        try await call(
            method: "project_checkout",
            payload: DaemonProjectCheckoutRequest(projectId: projectId)
        )
    }

    func syncStatus(projectId: String? = nil) async throws -> DaemonSyncStatus {
        guard let projectId else {
            return try await call(method: "sync_status", payload: EmptyPayload())
        }
        return try await call(
            method: "project_sync_status",
            payload: DaemonProjectSyncStatusRequest(projectId: projectId)
        )
    }

    func projectStorage(_ projectId: String) async throws -> DaemonProjectStorage {
        try await call(
            method: "project_storage",
            payload: DaemonProjectStorageRequest(projectId: projectId)
        )
    }

    func replaceProjectStorage(_ request: DaemonProjectStorageReplaceRequest) async throws
        -> DaemonProjectStorageMove {
        try await call(method: "replace_project_storage", payload: request)
    }

    func projectStorageMove(_ moveId: String) async throws -> DaemonProjectStorageMove {
        try await call(
            method: "project_storage_move",
            payload: DaemonProjectStorageMoveRequest(moveId: moveId)
        )
    }

    func resetProjectStorage(_ request: DaemonProjectStorageResetRequest) async throws
        -> DaemonProjectStorageMove {
        try await call(method: "reset_project_storage", payload: request)
    }

    func clearProjectCache(_ request: DaemonProjectCacheClearRequest) async throws
        -> DaemonProjectStorage {
        try await call(method: "clear_project_cache", payload: request)
    }

    func retrySync(
        channel: String = "all",
        projectId: String? = nil
    ) async throws -> DaemonRetryResponse {
        if let projectId {
            return try await call(
                method: "project_retry_sync",
                payload: DaemonProjectSyncRetryRequest(
                    projectId: projectId,
                    channel: channel
                ),
                timeout: 300
            )
        }
        return try await call(
            method: "retry_sync",
            payload: DaemonSyncRetryRequest(channel: channel),
            timeout: 300
        )
    }

    func listDrafts(_ query: DaemonDraftListQuery = .init(limit: 200)) async throws -> DaemonDraftListResponse {
        try await call(method: "list_drafts", payload: query)
    }

    func draft(_ draftId: String) async throws -> DaemonDraftDetail {
        try await call(method: "get_draft", payload: DaemonDraftDetailRequest(draftId: draftId))
    }

    func store(_ request: DaemonDraftOperationRequest) async throws -> DaemonDraftOperationResponse {
        try await call(method: "desktop_store_draft_operation", payload: request)
    }

    func listRetrievalRuns(_ request: RetrievalRunListRequest) async throws
        -> RetrievalRunListResponse {
        try await call(method: "list_retrieval_runs", payload: request)
    }

    func listRecalls(_ request: ListRecallsRequest) async throws -> ListRecallsResponse {
        try await call(method: "list_recalls", payload: request)
    }

    func recallFragment(_ request: GetRecallFragmentRequest) async throws
        -> GetRecallFragmentResponse {
        try await call(method: "get_recall_fragment", payload: request)
    }

    func retrievalRun(_ runId: String) async throws -> RetrievalRunDetail {
        try await call(
            method: "get_retrieval_run",
            payload: RetrievalRunRequest(runId: runId)
        )
    }

    func createEvaluationCase(_ request: CreateEvaluationCaseRequest) async throws
        -> EvaluationCaseDetail {
        try await call(method: "create_evaluation_case", payload: request)
    }

    func resolveEvaluationCase(_ request: ResolveEvaluationCaseRequest) async throws
        -> EvaluationCaseDetail {
        try await call(method: "resolve_evaluation_case", payload: request)
    }

    func clearRetrievalRuns(projectId: String?) async throws -> ClearRetrievalRunsResponse {
        try await call(
            method: "clear_retrieval_runs",
            payload: ClearRetrievalRunsRequest(projectId: projectId)
        )
    }

    func exportEvaluationSet(projectId: String?, caseIds: [String] = []) async throws
        -> ExportEvaluationSetResponse {
        try await call(
            method: "export_evaluation_set",
            payload: ExportEvaluationSetRequest(projectId: projectId, caseIds: caseIds)
        )
    }

    func serverRequest(_ request: DaemonServerRequest) async throws -> DaemonServerResponse {
        try await call(method: "server_request", payload: request)
    }

    func call<Payload: Encodable & Sendable, Response: Decodable & Sendable>(
        method: String,
        payload: Payload,
        timeout: TimeInterval = 30
    ) async throws -> Response {
        try await ClientDiagnostics.operation(layer: "xpc", method: method) {
            try await performCall(method: method, payload: payload, timeout: timeout)
        }
    }

    private func performCall<Payload: Encodable & Sendable, Response: Decodable & Sendable>(
        method: String, payload: Payload, timeout: TimeInterval
    ) async throws -> Response {
        let encoder = JSONCoding.encoder()
        let payloadData = try encoder.encode(payload)
        let payloadObject = try JSONSerialization.jsonObject(with: payloadData)
        let requestObject: [String: Any] = ["method": method, "payload": payloadObject, "request_id": ClientDiagnostics.requestID ?? ""]
        guard JSONSerialization.isValidJSONObject(requestObject) else {
            throw DaemonXPCError.invalidRequest
        }
        let requestData = try JSONSerialization.data(withJSONObject: requestObject)
        guard let requestJSON = String(data: requestData, encoding: .utf8) else {
            throw DaemonXPCError.invalidRequest
        }

        let responseJSON = try await Self.send(requestJSON, to: serviceName, timeout: timeout)
        guard let responseData = responseJSON.data(using: .utf8),
              let object = try JSONSerialization.jsonObject(with: responseData) as? [String: Any],
              let ok = object["ok"] as? Bool else {
            throw DaemonXPCError.invalidReply
        }
        if !ok {
            let errorObject = object["error"] ?? [:]
            let errorData = try JSONSerialization.data(withJSONObject: errorObject)
            if let error = try? JSONCoding.decoder().decode(APIErrorPayload.self, from: errorData) {
                throw DaemonXPCError.daemon(error)
            }
            throw DaemonXPCError.invalidReply
        }
        guard let payloadObject = object["payload"] else {
            throw DaemonXPCError.invalidReply
        }
        let decodedPayloadData = try JSONSerialization.data(withJSONObject: payloadObject)
        return try JSONCoding.decoder().decode(Response.self, from: decodedPayloadData)
    }

    private static func send(
        _ requestJSON: String,
        to serviceName: String,
        timeout: TimeInterval
    ) async throws -> String {
        let request = DaemonXPCRequestState()
        let response = try await withTaskCancellationHandler {
            try Task.checkCancellation()
            return try await withCheckedThrowingContinuation { continuation in
                guard request.start(continuation) else { return }
                let connection = xpc_connection_create_mach_service(serviceName, replyQueue, 0)
                xpc_connection_set_event_handler(connection) { event in
                    if xpc_get_type(event) == XPC_TYPE_ERROR {
                        let desc = xpc_dictionary_get_string(event, XPC_ERROR_KEY_DESCRIPTION)
                            .map(String.init(cString:))
                        request.fail(.connectionFailed(detail: desc))
                    }
                }
                guard request.attach(connection) else { return }
                xpc_connection_activate(connection)

                let message = xpc_dictionary_create(nil, nil, 0)
                xpc_dictionary_set_string(message, "request_json", requestJSON)
                xpc_connection_send_message_with_reply(connection, message, replyQueue) { reply in
                    guard xpc_get_type(reply) != XPC_TYPE_ERROR else {
                        let desc = xpc_dictionary_get_string(reply, XPC_ERROR_KEY_DESCRIPTION)
                            .map(String.init(cString:))
                        request.fail(.connectionFailed(detail: desc))
                        return
                    }
                    guard xpc_get_type(reply) == XPC_TYPE_DICTIONARY,
                          let responsePointer = xpc_dictionary_get_string(reply, "response_json") else {
                        request.fail(.invalidReply)
                        return
                    }
                    request.succeed(String(cString: responsePointer))
                }
                replyQueue.asyncAfter(deadline: .now() + timeout) {
                    request.fail(.requestTimedOut(timeout: timeout))
                }
            }
        } onCancel: {
            request.cancel()
        }
        try Task.checkCancellation()
        return response
    }
}

struct DaemonStartupReadiness: Sendable {
    typealias HealthCheck = @Sendable (TimeInterval) async throws -> DaemonHealth

    let timeout: Duration
    let retryInterval: Duration
    let requestTimeout: Duration

    init(
        timeout: Duration = .seconds(30),
        retryInterval: Duration = .milliseconds(150),
        requestTimeout: Duration = .seconds(3)
    ) {
        self.timeout = timeout
        self.retryInterval = retryInterval
        self.requestTimeout = requestTimeout
    }

    func waitForHealth(_ check: HealthCheck) async throws -> DaemonHealth {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        var lastError: DaemonXPCError = .connectionFailed()

        while clock.now < deadline {
            let remaining = clock.now.duration(to: deadline)
            do {
                return try await check(min(requestTimeout, remaining).timeInterval)
            } catch is CancellationError {
                throw CancellationError()
            } catch let error as DaemonXPCError {
                switch error {
                case .connectionFailed, .requestTimedOut:
                    lastError = error
                case .invalidRequest, .invalidReply, .daemon:
                    throw error
                }
            }

            let retryRemaining = clock.now.duration(to: deadline)
            guard retryRemaining > .zero else { break }
            try await Task.sleep(for: min(retryInterval, retryRemaining))
        }

        throw lastError
    }
}

private extension Duration {
    var timeInterval: TimeInterval {
        let parts = components
        return TimeInterval(parts.seconds) + TimeInterval(parts.attoseconds) / 1_000_000_000_000_000_000
    }
}

final class DaemonXPCRequestState: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<String, Error>?
    private var connection: xpc_connection_t?
    private var isCancelled = false

    @discardableResult
    func start(_ continuation: CheckedContinuation<String, Error>) -> Bool {
        let didStart = lock.withLock {
            guard !isCancelled, self.continuation == nil else { return false }
            self.continuation = continuation
            return true
        }
        if !didStart {
            continuation.resume(throwing: CancellationError())
        }
        return didStart
    }

    @discardableResult
    func attach(_ connection: xpc_connection_t) -> Bool {
        let didAttach = lock.withLock {
            guard !isCancelled, continuation != nil else { return false }
            self.connection = connection
            return true
        }
        if !didAttach {
            xpc_connection_cancel(connection)
        }
        return didAttach
    }

    func succeed(_ response: String) {
        finish(.success(response))
    }

    func fail(_ error: DaemonXPCError) {
        finish(.failure(error))
    }

    func cancel() {
        finish(.failure(CancellationError()), markCancelled: true)
    }

    private func finish(_ result: Result<String, Error>, markCancelled: Bool = false) {
        let completion: (CheckedContinuation<String, Error>, xpc_connection_t?)? = lock.withLock {
            if markCancelled {
                isCancelled = true
            }
            guard let continuation else { return nil }
            self.continuation = nil
            let connection = self.connection
            self.connection = nil
            return (continuation, connection)
        }
        guard let (continuation, connection) = completion else { return }
        if let connection {
            xpc_connection_cancel(connection)
        }
        switch result {
        case .success(let response):
            continuation.resume(returning: response)
        case .failure(let error):
            continuation.resume(throwing: error)
        }
    }
}
