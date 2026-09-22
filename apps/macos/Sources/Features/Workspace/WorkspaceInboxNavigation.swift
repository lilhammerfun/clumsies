import Foundation

extension WorkspaceCoordinator {
    private func openInboxSharedUpdates(projectId: String) async throws {
        let authority = context.authorityGeneration
        try await selectInboxProject(projectId)
        // A server notification can arrive before this Mac has downloaded its commit.
        _ = try await context.daemon.retrySync(channel: "commits", projectId: projectId)
        try context.ensureAuthority(authority)
        let status = try await context.daemon.syncStatus(projectId: projectId)
        if ["failed", "degraded"].contains(status.commitSync.state) {
            if let error = status.commitSync.lastError { throw error }
            throw ActionFailure(String(localized: "Couldn't download the remote changes. Try again when connected."))
        }
        try context.ensureAuthority(authority)
        guard context.activeProjectId == projectId else { throw CancellationError() }
        await sync.refreshStaleResourcesIfNeeded(sync: status)
        try context.ensureAuthority(authority)
        guard context.activeProjectId == projectId, navigation.selectedSection == .inbox else { throw CancellationError() }
        if let error = feedback.presentedBackgroundError,
           error.source == .staleResources(projectId: projectId) {
            throw ActionFailure(error.message)
        }
        revealInboxSharedUpdates(in: projectId)
    }

    func revealInboxSharedUpdates(in projectId: String) {
        guard context.activeProjectId == projectId, navigation.selectedSection == .inbox else { return }
        let changedIds = Set(catalog.staleResourceSnapshots.filter { $0.value.projectId == projectId }.keys)
        let changed = navigation.memoryItems.filter { changedIds.contains($0.id) }
            .sorted { $0.document.path.localizedStandardCompare($1.document.path) == .orderedAscending }
        navigation.searchQuery = ""
        navigation.selectedSection = .memory
        for item in changed { navigation.open(item, mode: .diff) }
        if let first = changed.first { navigation.open(first, mode: .diff) }
    }

    func openInboxDestination(_ destination: InboxDestination) async throws {
        let authority = context.authorityGeneration
        guard await flushPendingChanges() else {
            throw ActionFailure(String(localized: "Save the current edits before opening this notification."))
        }
        try context.ensureAuthority(authority)
        guard navigation.selectedSection == .inbox else { throw CancellationError() }
        switch destination {
        case .review(let reviewId):
            let detail = try await reviews.reviewDetail(reviewId)
            try context.ensureAuthority(authority)
            guard navigation.selectedSection == .inbox else { throw CancellationError() }
            let record = WorkspaceLoader.mapReview(detail)
            reviews.replaceReview(with: record)
            reviews.openReview(record)
        case .project(let projectId):
            try await selectInboxProject(projectId)
            guard navigation.selectedSection == .inbox else { throw CancellationError() }
            navigation.searchQuery = ""
            navigation.selectedSection = .memory
        case .manageLocalProjects:
            navigation.showsLocalProjectRecovery = true
        case .retrySync:
            let result = await refresh.retrySync(allProjects: true, reportFailure: false)
            try context.ensureAuthority(authority)
            if case .failed(let message) = result { throw ActionFailure(message) }
            await inbox.refresh()
        case .sharedChanges(let projectId):
            try await openInboxSharedUpdates(projectId: projectId)
        }
    }

    private func selectInboxProject(_ projectId: String) async throws {
        guard context.projects.contains(where: { $0.id == projectId }) else {
            throw ServerClientError.forbidden(String(localized: "This project is no longer accessible."))
        }
        let authority = context.authorityGeneration
        guard await flushPendingChanges() else {
            throw ActionFailure(String(localized: "Save the current edits before switching projects."))
        }
        try context.ensureAuthority(authority)
        guard navigation.selectedSection == .inbox else { throw CancellationError() }
        await selectProject(projectId)
        try context.ensureAuthority(authority)
        guard context.phase == .ready, context.activeProjectId == projectId,
              context.activeProject?.isLoaded == true, !context.isSwitchingMemoryContext else {
            throw ActionFailure(String(localized: "Finish the current Memory operation before opening this project."))
        }
    }
}
