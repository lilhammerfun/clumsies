import AppKit
import XCTest
@testable import Clumsies

@MainActor
final class AvatarImageCacheTests: XCTestCase {
    private let url = URL(string: "https://images.example.test/avatar.png")!

    func testConcurrentLoadsCoalesceAndLaterReadsHonorHTTPFreshness() async throws {
        let started = expectation(description: "First request started")
        let joined = expectation(description: "Second reader joined")
        var pending: CheckedContinuation<(Data, URLResponse), Error>?
        var requests: [URLRequest] = []
        let response = try imageResponse()
        let cache = AvatarImageCache(transport: URLSession(configuration: .ephemeral)) { request in
            requests.append(request)
            if requests.count == 1 {
                return try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
            }
            return response
        }
        defer { cache.reset() }
        let first = Task { await cache.image(for: url) }
        await fulfillment(of: [started], timeout: 1)
        let second = Task { joined.fulfill(); return await cache.image(for: url) }
        await fulfillment(of: [joined], timeout: 1)
        try XCTUnwrap(pending).resume(returning: response)
        let firstImage = await first.value
        let secondImage = await second.value
        XCTAssertNotNil(firstImage)
        XCTAssertTrue(firstImage === secondImage)
        XCTAssertEqual(requests.count, 1)

        let laterImage = await cache.image(for: url)
        XCTAssertNotNil(laterImage)
        XCTAssertEqual(requests.count, 2, "Decoded images must not bypass HTTP freshness indefinitely.")
        XCTAssertTrue(requests.allSatisfy { $0.cachePolicy == .useProtocolCachePolicy })
        XCTAssertTrue(requests.allSatisfy { $0.timeoutInterval == 15 })
    }

    func testResetRejectsOldResponseWithoutRemovingReplacementLoad() async throws {
        let firstStarted = expectation(description: "Old request")
        let newStarted = expectation(description: "New request")
        let joined = expectation(description: "New request joined")
        var pending: [CheckedContinuation<(Data, URLResponse), Error>] = []
        let urlCache = URLCache(memoryCapacity: 1_024 * 1_024, diskCapacity: 0)
        let session = AvatarImageCache.makeTransport(cache: urlCache)
        let response = try imageResponse()
        let cache = AvatarImageCache(transport: session) { _ in
            try await withCheckedThrowingContinuation {
                pending.append($0)
                if pending.count == 1 { firstStarted.fulfill() } else { newStarted.fulfill() }
            }
        }
        defer { cache.reset() }
        urlCache.storeCachedResponse(CachedURLResponse(response: response.1, data: response.0),
            for: URLRequest(url: url))
        XCTAssertNotNil(urlCache.cachedResponse(for: URLRequest(url: url)))

        let old = Task { await cache.image(for: url) }
        await fulfillment(of: [firstStarted], timeout: 1)
        cache.reset()
        XCTAssertNil(urlCache.cachedResponse(for: URLRequest(url: url)))
        let current = Task { await cache.image(for: url) }
        await fulfillment(of: [newStarted], timeout: 1)
        pending[0].resume(returning: response)
        let oldImage = await old.value
        XCTAssertNil(oldImage, "A cancelled loader may still complete its I/O; its result belongs to the old account.")
        let secondCurrent = Task { joined.fulfill(); return await cache.image(for: url) }
        await fulfillment(of: [joined], timeout: 1)
        XCTAssertEqual(pending.count, 2, "An old completion must not remove the replacement load.")
        pending[1].resume(returning: response)
        let currentImage = await current.value
        let joinedImage = await secondCurrent.value
        XCTAssertNotNil(currentImage)
        XCTAssertTrue(currentImage === joinedImage)
    }

    func testTransportUsesIsolatedBoundedCacheAndRejectsNonHTTPURLs() async {
        let urlCache = URLCache(memoryCapacity: 1_024 * 1_024, diskCapacity: 0)
        let session = AvatarImageCache.makeTransport(cache: urlCache)
        defer { session.invalidateAndCancel() }
        XCTAssertTrue(session.configuration.urlCache === urlCache)
        XCTAssertEqual(session.configuration.requestCachePolicy, .useProtocolCachePolicy)
        XCTAssertFalse(session.configuration.httpShouldSetCookies)
        XCTAssertNil(session.configuration.httpCookieStorage)
        XCTAssertNil(session.configuration.urlCredentialStorage)
        let cache = AvatarImageCache(transport: session) { _ in
            XCTFail("An unsupported avatar URL must not be fetched.")
            throw URLError(.unsupportedURL)
        }
        for value in ["file:///private/avatar.png", "data:image/png;base64,AAA", "https://user:secret@example.test/avatar"] {
            let image = await cache.image(for: URL(string: value)!)
            XCTAssertNil(image)
        }
        cache.reset()
    }

    func testCancelledReaderDoesNotLeaveCompletedImageOutsideHTTPCache() async throws {
        let started = expectation(description: "Reader started")
        var pending: CheckedContinuation<(Data, URLResponse), Error>?
        var reads = 0
        let response = try imageResponse()
        let cache = AvatarImageCache(transport: URLSession(configuration: .ephemeral)) { _ in
            reads += 1
            if reads > 1 { return response }
            return try await withCheckedThrowingContinuation { pending = $0; started.fulfill() }
        }
        defer { cache.reset() }
        let reader = Task { await cache.image(for: url) }
        await fulfillment(of: [started], timeout: 1)
        reader.cancel()
        try XCTUnwrap(pending).resume(returning: response)
        let cancelledImage = await reader.value
        XCTAssertNil(cancelledImage)
        let nextImage = await cache.image(for: url)
        XCTAssertNotNil(nextImage)
        XCTAssertEqual(reads, 2)
    }

    private func imageResponse() throws -> (Data, URLResponse) {
        let bitmap = try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1, pixelsHigh: 1,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 4, bitsPerPixel: 32))
        bitmap.setColor(.blue, atX: 0, y: 0)
        return (try XCTUnwrap(bitmap.representation(using: .png, properties: [:])),
            try XCTUnwrap(HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1",
                headerFields: ["Content-Type": "image/png", "Cache-Control": "no-cache"])))
    }
}
