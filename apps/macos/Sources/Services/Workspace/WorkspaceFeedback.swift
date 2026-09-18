import Combine
import Foundation

enum WorkspaceBackgroundErrorSource: Hashable {
    case organizationResources
    case staleResources(projectId: String)
    case projectRefresh(projectId: String)
}

struct WorkspaceBackgroundErrorPresentation {
    let source: WorkspaceBackgroundErrorSource
    let message: String
}

@MainActor
final class WorkspaceFeedback: ObservableObject {
    private let context: WorkspaceContext
    var isShowingOrganizationMemory: () -> Bool = { true }

    init(context: WorkspaceContext) {
        self.context = context
    }

    @Published var syncRetryErrors: [SyncRetryKey: String] = [:]
    @Published var errorMessage: String?

    var presentedSyncRetryErrorKey: SyncRetryKey?

    var dismissedBackgroundErrorSources: Set<WorkspaceBackgroundErrorSource> = []

    var presentedBackgroundError: WorkspaceBackgroundErrorPresentation?

    func dismissErrorMessage() {
        if let presentation = presentedBackgroundError,
           errorMessage == presentation.message {
            dismissedBackgroundErrorSources.insert(presentation.source)
        }
        errorMessage = nil
        presentedBackgroundError = nil
        presentedSyncRetryErrorKey = nil
    }

    func presentBackgroundError(
        _ message: String,
        source: WorkspaceBackgroundErrorSource
    ) {
        guard backgroundErrorIsRelevant(source),
              !dismissedBackgroundErrorSources.contains(source),
              errorMessage == nil || errorMessage == presentedBackgroundError?.message else {
            return
        }
        presentedBackgroundError = .init(source: source, message: message)
        errorMessage = message
    }

    func resolveBackgroundError(_ source: WorkspaceBackgroundErrorSource) {
        dismissedBackgroundErrorSources.remove(source)
        guard let presentation = presentedBackgroundError,
              presentation.source == source else {
            return
        }
        if errorMessage == presentation.message {
            errorMessage = nil
        }
        presentedBackgroundError = nil
    }

    func backgroundErrorIsRelevant(_ source: WorkspaceBackgroundErrorSource) -> Bool {
        switch source {
        case .organizationResources:
            return isShowingOrganizationMemory() && context.activeProjectId == nil
        case .staleResources(let projectId), .projectRefresh(let projectId):
            return context.activeProjectId == projectId
        }
    }

    func clearIrrelevantScopedErrorPresentation() {
        if let key = presentedSyncRetryErrorKey, key.projectId != self.context.activeProjectId {
            if errorMessage == syncRetryErrors[key] {
                errorMessage = nil
            }
            presentedSyncRetryErrorKey = nil
        }
        if let presentation = presentedBackgroundError,
           !backgroundErrorIsRelevant(presentation.source) {
            if errorMessage == presentation.message {
                errorMessage = nil
            }
            presentedBackgroundError = nil
        }
    }

    func resetBackgroundErrorPresentation() {
        if let presentation = presentedBackgroundError,
           errorMessage == presentation.message {
            errorMessage = nil
        }
        dismissedBackgroundErrorSources.removeAll()
        presentedBackgroundError = nil
    }
}
