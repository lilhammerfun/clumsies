import AppKit
import SwiftUI
import XCTest
@testable import Clumsies

final class MarkdownContentViewTests: XCTestCase {
    @MainActor
    func testFrontmatterWrapsInNarrowPreviews() throws {
        let source = """
        ---
        name: coding
        description: 完成软件开发与仓库交付全流程，包括实现、修复、重构、测试、提交、Pull Request、Issue、Copilot Review、仓库配置和版本发布。仅在任务涉及代码或工程交付物时使用。
        ---

        # Coding

        正文从这里开始。Frontmatter 应保持紧凑，并保留原始字段内容。
        """
        for scheme in [ColorScheme.light, .dark] {
            var heights: [CGFloat] = []
            for width: CGFloat in [380, 760] {
                let host = NSHostingView(rootView: MarkdownContentView(source: source)
                    .padding(24).frame(width: width)
                    .background(Color(nsColor: .textBackgroundColor))
                    .environment(\.colorScheme, scheme))
                let size = host.fittingSize
                XCTAssertEqual(size.width, width, accuracy: 1)
                XCTAssertLessThan(size.height, 420, "Frontmatter should not take over the preview as a heading.")
                heights.append(size.height)
                host.frame = NSRect(origin: .zero, size: size)
                let window = NSWindow(contentRect: host.frame, styleMask: [], backing: .buffered, defer: false)
                window.isReleasedWhenClosed = false
                window.appearance = NSAppearance(named: scheme == .light ? .aqua : .darkAqua)
                window.contentView = host
                defer { window.close() }
                for _ in 0..<3 {
                    host.layoutSubtreeIfNeeded()
                    RunLoop.current.run(until: Date().addingTimeInterval(0.05))
                }
                let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
                host.cacheDisplay(in: host.bounds, to: bitmap)
                let attachment = XCTAttachment(
                    data: try XCTUnwrap(bitmap.representation(using: .png, properties: [:])),
                    uniformTypeIdentifier: "public.png"
                )
                attachment.name = "Frontmatter-\(scheme)-\(Int(width))"
                attachment.lifetime = .keepAlways
                add(attachment)
            }
            XCTAssertGreaterThan(heights[0], heights[1], "Long metadata must wrap instead of clipping.")
        }
    }

    func testFrontmatterIsSeparatedFromMarkdownBody() {
        let metadata = """
        name: coding
        description: >-
          完成软件开发与仓库交付全流程。
          Keep **literal text** and <tags>.
        tools:
          - read
          - edit
        nested:
          separator: |
            ---
        """
        let body = "\n# Coding\n\nHeading\n---\n\nBody\n\n---\n"
        for newline in ["\n", "\r\n"] {
            for closing in ["---", "..."] {
                for prefix in ["", "\u{FEFF}"] {
                    let source = "\(prefix)---\n\(metadata)\n\(closing)\n\(body)"
                        .replacingOccurrences(of: "\n", with: newline)
                    let document = MarkdownPreviewDocument(source: source)
                    XCTAssertEqual(document.frontmatter, metadata.replacingOccurrences(of: "\n", with: newline))
                    XCTAssertEqual(document.body, body.replacingOccurrences(of: "\n", with: newline))
                }
            }
        }
    }

    func testOrdinaryMarkdownAndUnclosedFrontmatterStayIntact() {
        for source in [
            "", "# Coding\n\n---\nname: coding\n---",
            "Heading\n---\nBody", "---\n\n# Coding",
            "---\nname: coding\n", "---\nname: coding\n--- not a delimiter\n",
            "```yaml\n---\nname: coding\n---\n```",
            "\n---\nname: coding\n---\n# Coding"
        ] {
            let document = MarkdownPreviewDocument(source: source)
            XCTAssertNil(document.frontmatter, source)
            XCTAssertEqual(document.body, source)
        }
    }

    func testEmptyAndFrontmatterOnlyDocuments() {
        for closing in ["---", "..."] {
            let empty = MarkdownPreviewDocument(source: "---\n\(closing)\n# Coding")
            XCTAssertEqual(empty.frontmatter, "")
            XCTAssertEqual(empty.body, "# Coding")
            let metadataOnly = MarkdownPreviewDocument(source: "--- \t\nname: coding\n\(closing) \t")
            XCTAssertEqual(metadataOnly.frontmatter, "name: coding")
            XCTAssertEqual(metadataOnly.body, "")
        }
    }
}
