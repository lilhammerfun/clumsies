import Foundation

/// An inspected merge and the explicit choices still needed before it can be saved.
struct DraftResolution: Equatable, Sendable {
    let candidate: DraftReconciliationCandidate
    private(set) var state: ReconciliationResourceState
    private(set) var unresolvedFields: Set<String>
    private(set) var hasEdits = false

    init(candidate: DraftReconciliationCandidate) {
        self.candidate = candidate
        state = candidate.mergePreview?.state ?? candidate.proposedState ?? candidate.draftState
        unresolvedFields = Set(candidate.conflicts.map(\.field))
        if !sections.isEmpty { unresolvedFields.remove("content") }
    }

    var sections: [ContentConflictSection] {
        guard state.exists, let length = candidate.mergePreview?.markerLength else { return [] }
        return ContentConflictSection.parse(text, markerLength: length)
    }

    var canEditContent: Bool {
        !unresolvedFields.contains("content") && sections.isEmpty && !hasMarkers
    }

    var canSave: Bool {
        candidate.valid && unresolvedFields.isEmpty && !hasMarkers
            && (!state.exists || (state.content != nil && !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty))
    }

    var text: String { state.content?.primaryText ?? "" }
    var path: String { state.resource.path ?? "" }

    var previewText: String {
        var result = text as NSString
        for section in sections.reversed() {
            result = result.replacingCharacters(in: section.range,
                with: "⟪ Choose Remote or Draft for this change ⟫\n") as NSString
        }
        return result as String
    }

    private var hasMarkers: Bool {
        guard state.exists, let length = candidate.mergePreview?.markerLength else { return false }
        return ContentConflictSection.hasMarkers(in: text, length: length)
    }

    mutating func chooseContent(_ content: String, in section: ContentConflictSection) {
        guard sections.contains(where: { $0.range == section.range && $0.shared == section.shared && $0.proposed == section.proposed }) else { return }
        replaceContent((text as NSString).replacingCharacters(in: section.range, with: content))
    }

    mutating func editContent(_ content: String) {
        guard canEditContent else { return }
        replaceContent(content)
    }

    mutating func choosePath(_ path: String) {
        let resource = ServerDraftResourceReference(scope: state.resource.scope, id: state.resource.id, path: path)
        state = .init(exists: state.exists, resource: resource, content: state.content)
        unresolvedFields.remove("path")
        unresolvedFields.remove("path_occupied")
        hasEdits = true
    }

    /// A whole-file choice is explicit and intentionally replaces all pending decisions.
    mutating func chooseFile(_ version: ReconciliationResourceState) {
        state = version
        unresolvedFields = []
        hasEdits = true
    }

    private mutating func replaceContent(_ content: String) {
        guard let template = state.content else { return }
        state = .init(exists: state.exists, resource: state.resource,
                      content: template.replacingPrimaryText(with: content))
        hasEdits = true
    }
}
