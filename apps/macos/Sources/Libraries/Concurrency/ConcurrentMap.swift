private struct IndexedValue<Value: Sendable>: Sendable {
    let index: Int
    let value: Value
}

func concurrentMap<Input: Sendable, Output: Sendable>(
    _ values: [Input],
    maxConcurrent: Int = 12,
    transform: @escaping @Sendable (Input) async throws -> Output
) async throws -> [Output] {
    guard !values.isEmpty else { return [] }
    let limit = min(max(1, maxConcurrent), values.count)
    return try await withThrowingTaskGroup(of: IndexedValue<Output>.self) { group in
        var nextIndex = 0
        var output = [Output?](repeating: nil, count: values.count)

        func enqueue(_ index: Int) {
            let value = values[index]
            group.addTask {
                IndexedValue(index: index, value: try await transform(value))
            }
        }

        while nextIndex < limit {
            enqueue(nextIndex)
            nextIndex += 1
        }
        while let result = try await group.next() {
            output[result.index] = result.value
            if nextIndex < values.count {
                enqueue(nextIndex)
                nextIndex += 1
            }
        }
        return output.compactMap { $0 }
    }
}
