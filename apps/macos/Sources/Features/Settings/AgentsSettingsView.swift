import SwiftUI

struct AgentsSettingsView: View {
    var onCompleted: (() -> Void)?
    @StateObject private var model: AgentsSettingsModel

    init(model: @autoclosure @escaping () -> AgentsSettingsModel, onCompleted: (() -> Void)? = nil) {
        self.onCompleted = onCompleted
        _model = StateObject(wrappedValue: model())
    }

    var body: some View {
        VStack(spacing: 0) {
            if self.onCompleted != nil {
                VStack(spacing: 10) {
                    BrandLogoView(size: 52)
                    Text("Connect Your Agents")
                        .font(.system(size: 22, weight: .semibold))
                    Text("Choose the agents you use on this Mac. You can change this later in Settings.")
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .padding(.horizontal, 36)
                .padding(.top, 36)
            }
            Form {
                Section {
                    ForEach(ProjectAgentAdapterKind.allCases) { adapter in
                        VStack(alignment: .leading, spacing: 5) {
                            Toggle(adapter.title, isOn: Binding(
                                get: { self.model.selected.contains(adapter) },
                                set: { enabled in
                                    if enabled { self.model.selected.insert(adapter) }
                                    else { self.model.selected.remove(adapter) }
                                    if self.onCompleted == nil {
                                        Task { await self.model.save(adapter, enabled: enabled) }
                                    }
                                }
                            ))
                            .disabled(self.model.isWorking || !self.model.hasLoaded)
                            if adapter == .codex {
                                Text(self.model.codexDescription)
                                    .font(.caption).foregroundStyle(.secondary)
                            } else if self.onCompleted == nil,
                                      let setting = model.settings.first(where: { $0.adapter == adapter }),
                                      setting.enabled {
                                Text(setting.installed
                                    ? (adapter == .dsh ? "Enabled; MCP profile setup required" : "Installed for this Mac")
                                    : "Ready to install")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            if let setting = model.settings.first(where: { $0.adapter == adapter }),
                               setting.configured, setting.legacyRepositories > 0 {
                                Text("\(setting.legacyRepositories) old repository configuration(s) still need cleanup. Reconnect missing folders and retry.")
                                    .font(.caption).foregroundStyle(.orange)
                            }
                        }
                    }
                } header: {
                    Text("Agents on This Mac")
                } footer: {
                    Text("Install once for all projects. The repository binding selects which project’s Memory each agent uses.")
                }

                if self.onCompleted == nil {
                    Section {
                        Button("Repair Selected Integrations") { Task { if await self.model.saveSelection(refreshStatus: self.onCompleted == nil) { self.onCompleted?() } } }
                            .disabled(self.model.isWorking || !self.model.hasLoaded)
                    }
                }
                Section {
                    Text("After changing Codex, restart it and start a new task.")
                    if self.model.selected.contains(.dsh) {
                        Text("Register the dsh MCP entry in your dsh profile.")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)

                if let errorMessage = model.errorMessage {
                    Section {
                        Text(errorMessage).foregroundStyle(.red).textSelection(.enabled)
                        Button("Retry") { Task { await self.model.load() } }.disabled(self.model.isWorking)
                    }
                }
            }
            .formStyle(.grouped)
            .scrollContentBackground(self.onCompleted == nil ? .visible : .hidden)
            .font(.system(size: 13))
            .toggleStyle(.switch)

            if self.onCompleted != nil {
                HStack {
                    Button("Set Up Later") { self.onCompleted?() }
                        .disabled(self.model.isWorking)
                    if self.model.isWorking { ProgressView().controlSize(.small) }
                    Spacer()
                    Button(self.model.selected.isEmpty ? "Continue Without Adapters" : "Install and Continue") {
                        Task { if await self.model.saveSelection(refreshStatus: self.onCompleted == nil) { self.onCompleted?() } }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(self.model.isWorking || !self.model.hasLoaded)
                }
                .padding(24)
            }
        }
        .task { await self.model.load() }
    }

}
