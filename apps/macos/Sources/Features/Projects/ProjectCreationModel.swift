import Combine
import Foundation

enum ProjectMetadataValidation {
    static func isValid(name: String, description: String) -> Bool {
        let normalizedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedDescription = description.trimmingCharacters(in: .whitespacesAndNewlines)
        return !normalizedName.isEmpty
            && normalizedName.count <= 120
            && normalizedDescription.count <= 4_000
    }
}

@MainActor
final class ProjectCreationModel: ObservableObject {
    private let projects: ProjectService

    init(projects: ProjectService) { self.projects = projects }

    @Published var name = ""
    @Published var description = ""
    @Published var repositories: [URL] = []
    @Published var selectedBundleId: String?
    @Published private(set) var isCreating = false
    @Published private(set) var errorMessage: String?
    private let idempotencyKey = UUID().uuidString.lowercased()

    func create() async -> String? {
        guard !isCreating, ProjectMetadataValidation.isValid(name: name, description: description) else { return nil }
        isCreating = true
        errorMessage = nil
        do {
            return try await projects.createProject(
                name: name, description: description, idempotencyKey: idempotencyKey,
                repositoryPaths: repositories.map(\.path), bundleId: selectedBundleId
            )
        } catch {
            errorMessage = error.actionMessage
            isCreating = false
            return nil
        }
    }
}
