import Foundation

struct ContentConflictSection: Identifiable {
    let id: Int
    let range: NSRange
    let base: String
    let shared: String
    let proposed: String

    static func parse(_ text: String, markerLength: Int) -> [Self] {
        guard markerLength >= 7 else { return [] }
        let marker = markerLength
        let pattern = #"(?m)^<{\#(marker)} ours\n([\s\S]*?)^\|{\#(marker)} original\n([\s\S]*?)^={\#(marker)}\n([\s\S]*?)^>{\#(marker)} theirs(?:\n|$)"#
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return [] }
        let source = text as NSString
        return regex.matches(in: text, range: NSRange(location: 0, length: source.length)).map {
            .init(id: $0.range.location, range: $0.range,
                  base: source.substring(with: $0.range(at: 2)),
                  shared: source.substring(with: $0.range(at: 1)),
                  proposed: source.substring(with: $0.range(at: 3)))
        }
    }

    static func hasMarkers(in text: String, length: Int) -> Bool {
        guard length >= 7 else { return false }
        let prefixes = ["<", "|", "=", ">"].map { String(repeating: $0, count: length) }
        return text.split(separator: "\n").contains { line in prefixes.contains { line.hasPrefix($0) } }
    }
}
