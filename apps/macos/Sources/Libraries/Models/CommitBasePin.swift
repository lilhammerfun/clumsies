import Foundation

final class CommitBasePin: Hashable, Sendable {
    let commitId: String
    private let release: @Sendable () -> Void

    init(commitId: String, release: @escaping @Sendable () -> Void) {
        self.commitId = commitId
        self.release = release
    }

    deinit { release() }

    static func == (lhs: CommitBasePin, rhs: CommitBasePin) -> Bool { lhs.commitId == rhs.commitId }
    func hash(into hasher: inout Hasher) { hasher.combine(commitId) }
}
