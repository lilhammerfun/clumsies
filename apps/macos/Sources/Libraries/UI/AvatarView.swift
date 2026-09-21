import AppKit
import SwiftUI

@MainActor
private final class AvatarImageCache {
    static let shared = AvatarImageCache()

    private struct Load {
        let id: UUID
        let task: Task<NSImage?, Never>
    }

    private let images = NSCache<NSURL, NSImage>()
    private var loads: [URL: Load] = [:]

    private init() {
        images.countLimit = 128
    }

    func image(for url: URL) async -> NSImage? {
        if let image = images.object(forKey: url as NSURL) {
            return image
        }

        let load: Load
        if let existing = loads[url] {
            load = existing
        } else {
            var request = URLRequest(url: url)
            request.cachePolicy = .returnCacheDataElseLoad
            request.timeoutInterval = 15
            let task = Task<NSImage?, Never> {
                do {
                    let (data, response) = try await URLSession.shared.data(for: request)
                    guard let response = response as? HTTPURLResponse,
                          (200..<300).contains(response.statusCode) else {
                        ClientDiagnostics.record("avatar_failed", ["kind": "http_status", "status": String((response as? HTTPURLResponse)?.statusCode ?? 0)])
                        return nil
                    }
                    let image = NSImage(data: data)
                    if image == nil { ClientDiagnostics.record("avatar_failed", ["kind": "decode"]) }
                    return image
                } catch {
                    ClientDiagnostics.record("avatar_failed", ClientDiagnostics.failureFields(error))
                    return nil
                }
            }
            load = .init(id: UUID(), task: task)
            loads[url] = load
        }

        let image = await load.task.value
        if loads[url]?.id == load.id {
            if let image {
                images.setObject(image, forKey: url as NSURL)
            }
            loads[url] = nil
        }
        return image
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
        .task(id: avatarURL) {
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
            Text(String((account?.displayName ?? account?.email ?? "C").prefix(1)).uppercased())
                .font(.system(size: size == .small ? 10 : size == .regular ? 11 : 16, weight: .semibold))
                .foregroundStyle(Color.accentColor)
        }
        .frame(width: size.rawValue, height: size.rawValue)
    }
}
