import Darwin
import Foundation

enum ServerOriginError: LocalizedError, Equatable, Sendable {
    case empty
    case invalidURL
    case unsupportedScheme
    case insecureRemoteHTTP
    case credentialsNotAllowed
    case originOnly

    var errorDescription: String? {
        switch self {
        case .empty:
            "Enter the Server address."
        case .invalidURL:
            "Enter a complete Server address, such as https://clumsies.example.com."
        case .unsupportedScheme:
            "The Server address must use HTTPS. HTTP is allowed only for this Mac's loopback address."
        case .insecureRemoteHTTP:
            "Remote Server addresses must use HTTPS. HTTP is allowed only for localhost or a loopback IP address."
        case .credentialsNotAllowed:
            "The Server address cannot contain a user name or password."
        case .originOnly:
            "Enter only the Server origin, without a path, query, or fragment."
        }
    }
}

struct ServerOrigin: Equatable, Sendable {
    let url: URL

    init(validating input: String) throws {
        let trimmed = input.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { throw ServerOriginError.empty }
        guard var components = URLComponents(string: trimmed),
              let scheme = components.scheme?.lowercased(),
              let host = components.host,
              !host.isEmpty else {
            throw ServerOriginError.invalidURL
        }
        guard scheme == "http" || scheme == "https" else {
            throw ServerOriginError.unsupportedScheme
        }
        guard components.user == nil, components.password == nil else {
            throw ServerOriginError.credentialsNotAllowed
        }
        guard components.query == nil,
              components.fragment == nil,
              components.percentEncodedPath.isEmpty || components.percentEncodedPath == "/" else {
            throw ServerOriginError.originOnly
        }
        guard scheme == "https" || Self.isLoopback(host) else {
            throw ServerOriginError.insecureRemoteHTTP
        }

        components.scheme = scheme
        components.path = ""
        components.query = nil
        components.fragment = nil
        guard let normalized = components.url else { throw ServerOriginError.invalidURL }
        url = normalized
    }

    init(validating url: URL) throws {
        try self.init(validating: url.absoluteString)
    }

    static func isLoopback(_ rawHost: String) -> Bool {
        let host = rawHost
            .trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
            .lowercased()
            .trimmingCharacters(in: CharacterSet(charactersIn: "."))
        if host == "localhost" { return true }

        var ipv4 = in_addr()
        if host.withCString({ inet_pton(AF_INET, $0, &ipv4) }) == 1 {
            return UInt32(bigEndian: ipv4.s_addr) >> 24 == 127
        }

        var ipv6 = in6_addr()
        guard host.withCString({ inet_pton(AF_INET6, $0, &ipv6) }) == 1 else {
            return false
        }
        return withUnsafeBytes(of: &ipv6) { bytes in
            bytes.dropLast().allSatisfy { $0 == 0 } && bytes.last == 1
        }
    }
}

struct ServerOriginStore: @unchecked Sendable {
    static let defaultsKey = "ClumsiesServerOrigin"

    let defaults: UserDefaults
    let fallback: ServerOrigin

    init(defaults: UserDefaults = .standard, fallback: ServerOrigin) {
        self.defaults = defaults
        self.fallback = fallback
    }

    func load() -> ServerOrigin {
        guard let value = defaults.string(forKey: Self.defaultsKey),
              let origin = try? ServerOrigin(validating: value) else {
            return fallback
        }
        return origin
    }

    func save(_ origin: ServerOrigin) {
        defaults.set(origin.url.absoluteString, forKey: Self.defaultsKey)
    }
}
