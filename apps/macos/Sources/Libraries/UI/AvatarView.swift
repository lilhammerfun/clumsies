import AppKit
import SwiftUI

@MainActor
final class AvatarImageCache {
    private static var sharedInstance: AvatarImageCache?

    static var shared: AvatarImageCache {
        if let sharedInstance { return sharedInstance }
        let instance = AvatarImageCache(transport: makeTransport())
        sharedInstance = instance
        return instance
    }

    static func resetShared() { sharedInstance?.reset() }

    private struct Load {
        let id: UUID
        let task: Task<NSImage?, Never>
    }

    private var loads: [URL: Load] = [:]
    private var generation = UUID()
    private var transport: URLSession
    private let fetch: ((URLRequest) async throws -> (Data, URLResponse))?

    init(transport: URLSession, fetch: ((URLRequest) async throws -> (Data, URLResponse))? = nil) {
        self.transport = transport
        self.fetch = fetch
    }

    deinit { transport.invalidateAndCancel() }

    static func makeTransport(cache: URLCache? = nil) -> URLSession {
        let configuration = URLSessionConfiguration.default
        var directory = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first!
            .appending(path: Bundle.main.bundleIdentifier ?? ClumsiesIdentifiers.stableAppBundleIdentifier)
        if let developmentInstanceID = ClumsiesIdentifiers.developmentInstanceID {
            directory = directory.appending(components: "dev", developmentInstanceID)
        }
        directory = directory.appending(path: "avatars", directoryHint: .isDirectory)
        configuration.urlCache = cache ?? URLCache(memoryCapacity: 8 * 1_024 * 1_024,
            diskCapacity: 32 * 1_024 * 1_024, directory: directory)
        configuration.requestCachePolicy = .useProtocolCachePolicy
        configuration.httpShouldSetCookies = false
        configuration.httpCookieStorage = nil
        configuration.urlCredentialStorage = nil
        return URLSession(configuration: configuration)
    }

    func reset() {
        generation = UUID()
        loads.values.forEach { $0.task.cancel() }
        loads.removeAll()
        let configuration = transport.configuration
        transport.invalidateAndCancel()
        configuration.urlCache?.removeAllCachedResponses()
        transport = URLSession(configuration: configuration)
    }

    func image(for url: URL) async -> NSImage? {
        guard !Task.isCancelled,
              ["https", "http"].contains(url.scheme?.lowercased() ?? ""),
              url.host != nil, url.user == nil, url.password == nil else { return nil }
        let requestGeneration = generation
        let load: Load
        if let existing = loads[url] {
            load = existing
        } else {
            var request = URLRequest(url: url)
            request.cachePolicy = .useProtocolCachePolicy
            request.timeoutInterval = 15
            let transport = transport
            let fetch = fetch
            let task = Task<NSImage?, Never> {
                do {
                    let result: (Data, URLResponse)
                    if let fetch { result = try await fetch(request) }
                    else { result = try await transport.data(for: request) }
                    try Task.checkCancellation()
                    let (data, response) = result
                    guard let response = response as? HTTPURLResponse,
                          (200..<300).contains(response.statusCode) else {
                        ClientDiagnostics.record("avatar_failed", ["kind": "http_status", "status": String((response as? HTTPURLResponse)?.statusCode ?? 0)])
                        return nil
                    }
                    let image = NSImage(data: data)
                    if image == nil { ClientDiagnostics.record("avatar_failed", ["kind": "decode"]) }
                    return image
                } catch {
                    if !error.isUserCancellation {
                        ClientDiagnostics.record("avatar_failed", ClientDiagnostics.failureFields(error))
                    }
                    return nil
                }
            }
            load = .init(id: UUID(), task: task)
            loads[url] = load
        }

        let image = await load.task.value
        guard generation == requestGeneration else { return nil }
        if loads[url]?.id == load.id {
            loads[url] = nil
        }
        return Task.isCancelled ? nil : image
    }
}

struct AvatarView: View {
    enum Size: CGFloat {
        case small = 20
        case regular = 24
        case large = 34
    }

    let account: UserReference?
    var size: Size = .regular
    @State private var image: NSImage?

    private var avatarURL: URL? {
        account?.avatarUrl.flatMap(URL.init(string:))
    }

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFill()
                    .frame(width: size.rawValue, height: size.rawValue)
                    .clipped()
            } else {
                fallback
            }
        }
        .frame(width: size.rawValue, height: size.rawValue)
        .clipShape(Circle())
        .task(id: account) {
            if image != nil {
                image = nil
            }
            guard let avatarURL else { return }
            let loaded = await AvatarImageCache.shared.image(for: avatarURL)
            guard !Task.isCancelled else { return }
            image = loaded
        }
    }

    private var fallback: some View {
        ZStack {
            Color.accentColor.opacity(0.2)
            Text(String((account?.identityLabel ?? "C").prefix(1)).uppercased())
                .font(.system(size: size == .small ? 10 : size == .regular ? 11 : 16, weight: .semibold))
                .foregroundStyle(Color.accentColor)
        }
        .frame(width: size.rawValue, height: size.rawValue)
    }
}
