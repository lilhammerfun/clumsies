import Foundation

@MainActor
final class ProjectSelectionSideEffectGate {
    private let mutex = AsyncMutex()

    func run<Output>(_ operation: () async throws -> Output) async rethrows -> Output {
        await mutex.lock()
        do {
            let output = try await operation()
            await mutex.unlock()
            return output
        } catch {
            await mutex.unlock()
            throw error
        }
    }
}
