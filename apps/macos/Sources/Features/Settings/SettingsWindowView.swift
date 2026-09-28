import SwiftUI

struct SettingsIcon: View {
    let symbol: String
    var size: CGFloat = 16

    var body: some View {
        Image(systemName: symbol)
            .symbolRenderingMode(.monochrome)
            .foregroundStyle(.primary)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

struct SettingsWindowView: View {
    @EnvironmentObject private var workspaceFeedback: WorkspaceFeedback
    @EnvironmentObject private var agentIntegration: AgentIntegrationService
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var administration: AdministrationModel
    @ObservedObject var softwareUpdateController: SoftwareUpdateController
    @ObservedObject var navigation: SettingsNavigation
    @State private var showsAccountSecurity = false
    let onShowLogs: () -> Void
    let onRestart: () -> Void

    private var canShowOrganization: Bool {
        workspaceContext.canAdministerOrganization && workspaceContext.phase != .authenticationRequired
    }

    private var pageTitle: String { navigation.destination.title }

    var body: some View {
        NavigationSplitView {
            sidebar
                .toolbar(removing: .sidebarToggle)
                .navigationSplitViewColumnWidth(220)
        } detail: {
            Group {
                if #available(macOS 15, *), navigation.destination == .pane(.general) {
                    detail.toolbar(removing: .title)
                } else {
                    detail
                }
            }
                .id(navigation.contentGeneration)
                .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
                .background(Color(nsColor: .windowBackgroundColor))
                .navigationTitle(pageTitle)
                .toolbar {
                    ToolbarItemGroup(placement: .navigation) {
                        Button { navigation.goBack() } label: { Image(systemName: "chevron.left") }
                            .disabled(!navigation.canGoBack)
                            .toolbarHelp(String(localized: "Back"))
                            .accessibilityLabel("Back")
                            .keyboardShortcut("[", modifiers: .command)
                        Button { navigation.goForward() } label: { Image(systemName: "chevron.right") }
                            .disabled(!navigation.canGoForward)
                            .toolbarHelp(String(localized: "Forward"))
                            .accessibilityLabel("Forward")
                            .keyboardShortcut("]", modifiers: .command)
                    }
                    if navigation.isSaving {
                        ToolbarItem { ProgressView().controlSize(.small).toolbarHelp(String(localized: "Saving changes…")) }
                    }
                }
        }
        .automaticRefresh(id: navigation.destination) {
            guard !navigation.hasUnsavedChanges, !navigation.isSaving, !showsAccountSecurity,
                  !workspaceContext.isMutatingAdministration else { return }
            let destination = navigation.destination
            let canApply: @MainActor () -> Bool = { !navigation.hasUnsavedChanges && !navigation.isSaving
                && !workspaceContext.isMutatingAdministration && navigation.destination == destination }
            switch destination {
            case .organization(let section):
                await administration.refreshInBackground(section: section, canApply: canApply)
            case .pane(.organization):
                await administration.refreshInBackground(section: .organization, canApply: canApply)
            default: break
            }
        }
        .sheet(isPresented: $showsAccountSecurity) { AccountSecurityView() }
        .navigationSplitViewStyle(.balanced)
        .pageFeedback(administration.statusMessage, isStatus: true, dismiss: { administration.statusMessage = nil })
        .feedbackHost(error: workspaceFeedback.errorMessage, dismiss: workspaceFeedback.dismissErrorMessage)
        .toolbar(removing: .sidebarToggle)
        .font(.system(size: 13))
        .toggleStyle(.switch)
        .alert("Discard unsaved changes?", isPresented: Binding(
            get: { navigation.pendingDestination != nil },
            set: { if !$0 { navigation.pendingDestination = nil } }
        )) {
            Button("Keep Editing", role: .cancel) { navigation.pendingDestination = nil }
            Button("Discard Changes", role: .destructive) { navigation.discardAndNavigate() }
        } message: {
            Text("Your changes have not been saved.")
        }
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            ClassicSearchField(text: $navigation.query, prompt: String(localized: "Search"), width: 192,
                               accessibilityIdentifier: "settings-search")
                .frame(height: 28)
                .padding(.horizontal, 14)
                .padding(.top, 10)
                .padding(.bottom, 12)
            if navigation.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                List(selection: Binding<SettingsPane?>(
                    get: { navigation.destination.pane },
                    set: { if let pane = $0 { navigation.navigate(to: .pane(pane)) } }
                )) {
                    Section {
                        if let account = workspaceContext.account {
                            HStack(spacing: 10) {
                                AvatarView(account: account, size: .large)
                                    .accessibilityHidden(true)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(account.identityLabel)
                                        .fontWeight(.semibold)
                                        .lineLimit(1)
                                    Text(workspaceContext.organization?.name ?? "Clumsies")
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                            }
                            .padding(.vertical, 6)
                            .selectionDisabled()
                        }
                    }
                    Section {
                        Button("Login methods…") { showsAccountSecurity = true }
                            .disabled(workspaceContext.account == nil)
                        ForEach(SettingsPane.allCases.filter { $0 != .organization || canShowOrganization }) { pane in
                            HStack(spacing: 8) {
                                SettingsIcon(symbol: pane.systemImage)
                                Text(pane.title)
                            }
                            .padding(.vertical, 2)
                            .tag(pane)
                            .accessibilityIdentifier("settings-pane-\(pane.rawValue)")
                        }
                    }
                }
                .listStyle(.sidebar)
            } else {
                List {
                    ForEach(SettingsDestination.search(navigation.query, canAdminister: canShowOrganization)) { destination in
                        Button {
                            navigation.navigate(to: destination)
                        } label: {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(destination.title)
                                    .foregroundStyle(.primary)
                                Text(destination.pane.title)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .padding(.vertical, 4)
                    }
                    if SettingsDestination.search(navigation.query, canAdminister: canShowOrganization).isEmpty {
                        Text("No Results").foregroundStyle(.secondary)
                    }
                }
                .listStyle(.sidebar)
            }
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch navigation.destination {
        case .pane(.general):
            GeneralSettingsView(softwareUpdateController: softwareUpdateController, onRestart: onRestart)
        case .pane(.agent):
            AgentsSettingsView(model: AgentsSettingsModel(context: workspaceContext, integration: agentIntegration))
        case .pane(.advanced):
            SupportSettingsView(onShowLogs: onShowLogs)
        case .pane(.organization):
            if canShowOrganization { organizationLanding }
        case .organization(let section):
            if canShowOrganization {
                AdministrationView(section: section,
                    onUnsavedChangesChange: { navigation.hasUnsavedChanges = $0 })
                    .id(section)
            }
        }
    }

    private var organizationLanding: some View {
        Form {
            OrganizationNameSection(onUnsavedChangesChange: { navigation.hasUnsavedChanges = $0 })
            Section {
                organizationLink(.members)
                organizationLink(.projects)
                organizationLink(.access)
                organizationLink(.audit)
            }
        }
        .formStyle(.grouped)
        .task { await administration.load(section: .organization) }
    }

    private func organizationLink(_ section: AdministrationSection) -> some View {
        let destination = SettingsDestination.organization(section)
        return Button {
            navigation.navigate(to: destination)
        } label: {
            HStack(spacing: 8) {
                SettingsIcon(symbol: section.symbol)
                Text(destination.title)
                Spacer()
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
            .foregroundStyle(.primary)
            .frame(minHeight: 22)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("settings-organization-\(section.rawValue)")
    }
}
