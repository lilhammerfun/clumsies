import SwiftUI

struct ToolbarFilterMenu<MenuContent: View>: View {
    let selectionTitle: String
    let isLoading: Bool

    private let menuContent: () -> MenuContent

    init(
        selectionTitle: String,
        isLoading: Bool = false,
        @ViewBuilder menuContent: @escaping () -> MenuContent
    ) {
        self.selectionTitle = selectionTitle
        self.isLoading = isLoading
        self.menuContent = menuContent
    }

    var body: some View {
        Menu {
            menuContent()
        } label: {
            HStack(spacing: 6) {
                if isLoading {
                    ProgressView()
                        .controlSize(.mini)
                } else {
                    Image(systemName: "line.3.horizontal.decrease")
                }

                Text(selectionTitle)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: 180, alignment: .leading)
            }
        }
        .frame(maxWidth: 220, alignment: .leading)
    }
}

struct ProjectFilterMenu: View {
    let projects: [ProjectState]
    let selectedProjectId: String?
    let unscopedTitle: String?
    let unscopedSystemImage: String?
    let isLoading: Bool
    let help: String
    let onCreate: (() -> Void)?
    let onSelect: (String?) -> Void

    private var selectionTitle: String {
        projects.first { $0.id == selectedProjectId }?.name
            ?? unscopedTitle
            ?? "Select Project"
    }

    var body: some View {
        ToolbarFilterMenu(selectionTitle: selectionTitle, isLoading: isLoading) {
            if let unscopedTitle {
                Button {
                    guard selectedProjectId != nil else { return }
                    onSelect(nil)
                } label: {
                    if selectedProjectId == nil {
                        Label(unscopedTitle, systemImage: "checkmark")
                    } else if let unscopedSystemImage {
                        Label(unscopedTitle, systemImage: unscopedSystemImage)
                    } else {
                        Text(unscopedTitle)
                    }
                }
                .disabled(isLoading)

                if !projects.isEmpty {
                    Divider()
                }
            }

            if projects.isEmpty {
                Button("No Projects") {}
                    .disabled(true)
            } else {
                ForEach(projects) { project in
                    Button {
                        guard project.id != selectedProjectId else { return }
                        onSelect(project.id)
                    } label: {
                        if project.id == selectedProjectId {
                            Label(project.name, systemImage: "checkmark")
                        } else {
                            Text(project.name)
                        }
                    }
                    .disabled(isLoading)
                }
            }

            if let onCreate {
                Divider()
                Button("New Project…", systemImage: "plus") {
                    onCreate()
                }
                .disabled(isLoading)
            }
        }
        .help(help)
        .accessibilityLabel("Project Filter")
        .accessibilityValue(selectionTitle)
    }
}
