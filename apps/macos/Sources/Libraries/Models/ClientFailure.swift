import Foundation

/// Only locally authored recovery instructions may pass through to the interface.
protocol UserFacingError: LocalizedError {}

/// A locally authored action result, including messages already normalized at a request boundary.
struct ActionFailure: UserFacingError {
    let message: String
    var errorDescription: String? { message }
    init(_ message: String) { self.message = message }
}

enum ClientFailure: Equatable, Sendable {
    case cancelled, connection, localService, timeout, service, authentication
    case forbidden, missing, conflict, rateLimited, invalidInput, invalidResponse, fileAccess, storageFull, other

    init(_ error: Error) {
        if error is CancellationError { self = .cancelled; return }
        let ns = error as NSError
        if ns.domain == NSCocoaErrorDomain {
            switch ns.code {
            case NSUserCancelledError: self = .cancelled; return
            case NSFileReadNoPermissionError, NSFileWriteNoPermissionError: self = .fileAccess; return
            case NSFileWriteOutOfSpaceError: self = .storageFull; return
            case NSFileNoSuchFileError, NSFileReadNoSuchFileError: self = .missing; return
            default: break
            }
        }
        if ns.domain == NSURLErrorDomain {
            switch ns.code {
            case NSURLErrorCancelled: self = .cancelled
            case NSURLErrorNotConnectedToInternet, NSURLErrorNetworkConnectionLost,
                 NSURLErrorCannotConnectToHost, NSURLErrorCannotFindHost,
                 NSURLErrorDNSLookupFailed, NSURLErrorTimedOut: self = .connection
            default: self = .invalidResponse
            }
            return
        }
        switch error {
        case let error as ServerClientError:
            switch error {
            case .response(let status, _): self.init(status: status)
            case .forbidden: self = .forbidden
            case .invalidResponse, .invalidPath: self = .invalidResponse
            }
        case let error as DaemonXPCError:
            switch error {
            case .connectionFailed: self = .localService
            case .requestTimedOut: self = .timeout
            case .invalidReply, .invalidRequest: self = .invalidResponse
            case .daemon(let payload): self.init(payload)
            }
        case let payload as APIErrorPayload: self.init(payload)
        case AuthenticationError.server(let status, _): self.init(status: status)
        case NativeServerSetupError.server(let status, _): self.init(status: status)
        case is DecodingError, is EncodingError: self = .invalidResponse
        default: self = .other
        }
    }

    init(status: Int) {
        switch status {
        case 400, 422: self = .invalidInput
        case 401: self = .authentication
        case 403: self = .forbidden
        case 404, 410: self = .missing
        case 408, 504: self = .connection
        case 409, 412: self = .conflict
        case 429: self = .rateLimited
        case 500...599: self = .service
        default: self = .other
        }
    }

    private init(_ payload: APIErrorPayload) {
        if let status = payload.details?.status { self.init(status: status); return }
        switch payload.code {
        case "server_request_failed":
            self = payload.details?.decode == true ? .invalidResponse : .connection
        case "unauthorized", "authentication_required", "server_authentication_required": self = .authentication
        case "forbidden", "permission_denied": self = .forbidden
        case "not_found", "resource_not_found", "memory_resource_not_found": self = .missing
        case "conflict", "version_conflict", "draft_version_conflict": self = .conflict
        case "invalid_request", "invalid_params", "validation_error": self = .invalidInput
        case "rate_limited": self = .rateLimited
        case "invalid_json": self = .invalidResponse
        case "daemon_ipc_failed", "launchctl_failed": self = .localService
        default: self = .other
        }
    }

    var isServiceFailure: Bool {
        [.connection, .localService, .timeout, .service, .authentication, .rateLimited].contains(self)
    }

    var canRetryReceipt: Bool { [.connection, .localService, .timeout, .service, .rateLimited].contains(self) }

    var message: String {
        switch self {
        case .cancelled: String(localized: "The operation was cancelled.")
        case .connection: String(localized: "Clumsies can't connect to the server right now.")
        case .localService: String(localized: "The local service is unavailable. Reopen Clumsies to reconnect.")
        case .timeout: String(localized: "The service didn't respond in time. Try again.")
        case .service: String(localized: "The server is temporarily unavailable. Try again later.")
        case .authentication: String(localized: "Your session has expired. Sign in again to continue.")
        case .forbidden: String(localized: "You no longer have permission to perform this action. Refresh to check your access.")
        case .missing: String(localized: "This item is no longer available. It may have been removed or your access may have changed.")
        case .conflict: String(localized: "This item changed since you opened it. Check the latest version before trying again.")
        case .rateLimited: String(localized: "The server is busy. Wait a moment before trying again.")
        case .invalidInput: String(localized: "The request couldn't be accepted. Check the information you entered before trying again.")
        case .invalidResponse: String(localized: "Clumsies couldn't load this information. Try again or check for an app update.")
        case .fileAccess: String(localized: "Clumsies can't access this location. Choose a folder you have permission to use.")
        case .storageFull: String(localized: "There isn't enough space to save your changes. Free up space on this Mac and try again.")
        case .other: String(localized: "This operation couldn't be completed. Try again. If it keeps failing, use Settings to open the logs.")
        }
    }
}

extension Error {
    var isUserCancellation: Bool { ClientFailure(self) == .cancelled }

    var userFacingMessage: String {
        // Local validation and recovery instructions are useful; wire payloads are not UI copy.
        if let authored = self as? UserFacingError, let message = authored.errorDescription { return message }
        if let error = self as? ServerClientError, case .forbidden(let message) = error { return message }
        return ClientFailure(self).message
    }

    var actionMessage: String? { isUserCancellation ? nil : userFacingMessage }

    /// A refresh with retained content must not repeat an application connection failure.
    var backgroundMessage: String? {
        isUserCancellation || ClientFailure(self).isServiceFailure ? nil : userFacingMessage
    }
}
