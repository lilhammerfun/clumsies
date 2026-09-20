import MarkdownUI
import SwiftUI

struct MemoryGuidelinesSetupView: View {
    @EnvironmentObject private var workspaceContext: WorkspaceContext
    @EnvironmentObject private var memoryModel: MemoryModel
    @State private var preview: GuidelinesPreviewContent?
    @StateObject private var model: MemoryGuidelinesModel

    init(model: @autoclosure @escaping () -> MemoryGuidelinesModel) {
        _model = StateObject(wrappedValue: model())
    }

    var body: some View {
        Group {
            if self.model.isLoading {
                ContentLoadingView(title: String(localized: "Checking Memory Guidelines…"))
            } else if let error = model.error {
                ContentUnavailableView {
                    Label("Memory Guidelines Unavailable", systemImage: "doc.badge.ellipsis")
                } description: {
                    Text(error)
                } actions: {
                    Button("Try Again") { Task { self.preview = nil; await self.model.prepare() } }
                }
            } else if let setup = model.setup {
                ContentUnavailableView {
                    Label(self.title(for: setup), systemImage: "doc.text")
                } description: {
                    Text(self.description(for: setup))
                        .frame(maxWidth: 440)
                } actions: {
                    VStack(spacing: 12) {
                        Button(self.actionTitle(for: setup)) { Task { await self.model.adopt() } }
                            .buttonStyle(.borderedProminent)
                            .keyboardShortcut(.defaultAction)
                            .disabled(!self.model.canAdopt(setup) || self.model.isAdopting)
                        if setup.action == .createDefault {
                            Button("Preview guidelines and their sources") {
                                do {
                                    self.preview = GuidelinesPreviewContent(
                                        documents: try MemoryGuidelines.defaultDocuments(occupiedPaths: setup.occupiedPaths)
                                    )
                                } catch {
                                    self.model.error = error.localizedDescription
                                }
                            }
                            .buttonStyle(.link)
                            .disabled(self.model.isAdopting)
                            Text("Creates CLUMSIES.md and starter folders for knowledge, procedures, and lessons.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        if case .useOrganization = setup.action, !self.model.canAdopt(setup) {
                            Text("Ask a project administrator to add your organization's guidelines.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        if self.model.destinationChanged {
                            Text("Your memory guidelines changed. Review the current option above.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        if self.model.isAdopting {
                            ProgressView().controlSize(.small)
                        }
                    }
                }
            }
        }
        .task(id: workspaceContext.activeProjectId) { self.preview = nil; await self.model.prepare() }
        .sheet(item: $preview) { content in
            MemoryGuidelinesPreview(documents: content.documents) {
                self.preview = nil
                Task { await self.model.adopt() }
            }
        }
    }

    private func title(for setup: MemoryGuidelinesSetup) -> String {
        switch setup.action {
        case .createDefault: String(localized: "Give your memory a starting point")
        case .useOrganization: String(localized: "Use your team's memory guidelines")
        case .open: String(localized: "Your memory guidelines are ready")
        }
    }

    private func description(for setup: MemoryGuidelinesSetup) -> String {
        switch setup.action {
        case .createDefault:
            String(localized: "Memory guidelines tell agents what to remember and how to keep it useful. Start with our defaults and make them your own.")
        case .useOrganization:
            String(localized: "Your organization already has memory guidelines at \(setup.path). Use them in this project.")
        case .open:
            String(localized: "Your guidelines at \(setup.path) tell agents how to organize, update, and retire knowledge. You can edit them at any time.")
        }
    }

    private func actionTitle(for setup: MemoryGuidelinesSetup) -> String {
        switch setup.action {
        case .createDefault: String(localized: "Set Up Guidelines")
        case .useOrganization: String(localized: "Use Team Guidelines")
        case .open: String(localized: "Open Guidelines")
        }
    }

}

private struct GuidelinesPreviewContent: Identifiable {
    let id = UUID()
    let documents: [EditableMemoryDocument]
}

private struct MemoryGuidelinesPreview: View {
    let documents: [EditableMemoryDocument]
    let onUse: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var selectedPath = MemoryGuidelines.defaultPath

    private var document: EditableMemoryDocument {
        documents.first { $0.path == self.selectedPath } ?? documents[0]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Memory Guidelines").font(.title2.bold())
            Text("Choose how agents maintain your memory. You can edit these guidelines at any time. App updates will preserve your changes.")
                .foregroundStyle(.secondary)
            Picker("Starter document", selection: self.$selectedPath) {
                ForEach(self.documents, id: \.path) { document in
                    Text(document.title).tag(document.path)
                }
            }
            .pickerStyle(.segmented)
            ScrollView {
                Markdown(self.document.body)
                    .markdownTheme(.gitHub)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
            }
            .background(.background)
            HStack {
                Text(self.document.path).font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Close") { self.dismiss() }.keyboardShortcut(.cancelAction)
                Button("Set Up Guidelines", action: self.onUse)
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding(24)
        .frame(width: 680, height: 640)
    }
}
