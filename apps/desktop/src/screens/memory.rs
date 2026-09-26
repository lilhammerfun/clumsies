//! The Memory screen: the Project's file tree beside the selected document.

use std::collections::BTreeSet;

use clumsiesd::{DaemonDraftSummary, DaemonLocalDraftStatus};
use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tree::TreeState;
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::memory_tree;
use crate::engine::{Checkout, MemoryDocument};
use crate::screens::document::{DocumentPane, Notice, PANE_HEADER, PaneContext};
use crate::ui::{self, Typography};

pub struct MemoryScreen {
    tree: Entity<TreeState>,
    /// The Project the documents belong to, which is what a draft names.
    project_id: Option<String>,
    documents: Vec<MemoryDocument>,
    /// The Project ref the documents resolved from; a new draft is based on it.
    commit_id: Option<String>,
    /// The document the pane holds, as an index into the documents.
    selected: Option<usize>,
    /// A selection the pane has not been given yet.
    ///
    /// A tree click arrives as an entity notification, which carries no window,
    /// and the editor needs one to take new text. The flag carries the load to
    /// the next frame, which has a window.
    pending_load: bool,
    /// The drafts open in this Project, which is what the tree marks and what
    /// the pane offers to review.
    drafts: Vec<DaemonDraftSummary>,
    pane: DocumentPane,
    /// Why the documents could not be read, when they could not be.
    error: Option<String>,
    /// Dropping a subscription cancels it, so the screen holds it.
    _selection: Subscription,
}

impl MemoryScreen {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
        checkout: Option<Checkout>,
        error: Option<String>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx).items(Vec::new()));
        let pane = DocumentPane::new(window, cx);
        // Selecting an entry notifies the tree state, not this view.
        let selection = cx.observe(&tree, |app, _tree, cx| {
            app.memory().follow_selection(cx);
            cx.notify();
        });
        let mut screen = Self {
            tree,
            project_id: None,
            documents: Vec::new(),
            commit_id: None,
            selected: None,
            pending_load: false,
            drafts: Vec::new(),
            pane,
            error: None,
            _selection: selection,
        };
        screen.set_checkout(checkout, error, cx);
        screen
    }

    /// Replaces the documents when the selected Project changes or its checkout
    /// moves, and selects the first document, which is what opening a Project
    /// does in the macOS client.
    pub fn set_checkout(
        &mut self,
        checkout: Option<Checkout>,
        error: Option<String>,
        cx: &mut Context<DesktopApp>,
    ) {
        let (project_id, commit_id, documents) = match checkout {
            Some(checkout) => (
                Some(checkout.project_id),
                checkout.commit_id,
                checkout.documents,
            ),
            None => (None, None, Vec::new()),
        };
        self.project_id = project_id;
        self.commit_id = commit_id;
        self.documents = documents;
        self.error = error;
        self.drafts.clear();
        self.selected = (!self.documents.is_empty()).then_some(0);
        self.pending_load = true;
        self.publish(cx);
    }

    /// The open drafts the daemon holds for this Project. It is asked after
    /// every store, because a store is what creates or advances a draft.
    pub fn set_drafts(&mut self, drafts: Vec<DaemonDraftSummary>, cx: &mut Context<DesktopApp>) {
        self.drafts = drafts;
        self.publish(cx);
    }

    pub fn set_notice(&mut self, notice: Option<Notice>) {
        self.pane.set_notice(notice);
    }

    pub fn selected_document(&self) -> Option<&MemoryDocument> {
        self.selected.and_then(|index| self.documents.get(index))
    }

    /// The draft carrying the selected document's edits, when it has one.
    pub fn selected_draft(&self) -> Option<&DaemonDraftSummary> {
        self.selected_document()
            .and_then(|document| self.draft_for(document))
    }

    pub fn commit_id(&self) -> Option<&str> {
        self.commit_id.as_deref()
    }

    pub fn pane(&self) -> &DocumentPane {
        &self.pane
    }

    pub fn pane_mut(&mut self) -> &mut DocumentPane {
        &mut self.pane
    }

    /// Reads the tree's selection into this screen. The text follows in
    /// `MemoryScreen::apply_pending_load`, which has the window the editor
    /// needs to take it.
    fn follow_selection(&mut self, cx: &App) {
        let path = self
            .tree
            .read(cx)
            .selected_entry()
            .map(|entry| entry.item().id.to_string());
        let index = path.and_then(|path| {
            self.documents
                .iter()
                .position(|document| document.path == path)
        });
        if index != self.selected {
            self.selected = index;
            self.pending_load = true;
        }
    }

    /// Gives the pane the selected document. This runs at the top of a frame,
    /// before the pane draws, so the editor already holds the new text.
    pub fn apply_pending_load(&mut self, window: &mut Window, cx: &mut Context<DesktopApp>) {
        if !std::mem::take(&mut self.pending_load) {
            return;
        }
        if let Some(document) = self.selected.and_then(|index| self.documents.get(index)) {
            self.pane.load(document, window, cx);
            self.pane.focus_editor(window, cx);
        }
        self.sync_draft();
    }

    /// What the pane needs to name the selected document.
    pub(crate) fn render_target(&self) -> Option<PaneContext<'_>> {
        let document = self.selected_document()?;
        Some(PaneContext {
            project_id: self.project_id.as_deref().unwrap_or_default(),
            commit_id: self.commit_id.as_deref(),
            document,
        })
    }

    /// The section's list column: the Project's Memory, as files, under a header
    /// row of its own saying which Project and offering to change it. Every pane
    /// carries its own header; this is the list's.
    pub fn list(&self, project: AnyElement, cx: &App) -> AnyElement {
        let header = div()
            .h_flex()
            .h(px(PANE_HEADER))
            .px_3()
            .gap_2()
            .items_center()
            .child(div().text_style(&ui::BODY).child("Memory"))
            .child(project);

        div()
            .v_flex()
            .h_full()
            .child(header)
            .child(ui::rule(cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .p_2()
                    .child(memory_tree::memory_tree(&self.tree, &self.drafted_paths())),
            )
            .into_any_element()
    }

    /// The section's detail: the work itself, under the document pane's own
    /// header.
    pub fn detail(&self, actions: Option<AnyElement>, cx: &mut Context<DesktopApp>) -> AnyElement {
        let Some(target) = self.render_target() else {
            let reason = match &self.error {
                Some(error) => ui::message(error.clone(), cx.theme().danger),
                None => ui::message(
                    "This Project has no Memory yet.",
                    cx.theme().muted_foreground,
                ),
            };
            return div()
                .v_flex()
                .flex_1()
                .h_full()
                .min_w(px(0.))
                .p_4()
                .child(reason)
                .into_any_element();
        };
        // h_flex centers the cross axis, so a column in a row takes its content
        // height unless it asks for h_full(); the scroll regions inside need the
        // row's height to resolve against.
        self.pane.detail(Some(target), actions, cx)
    }

    /// Rebuilds what the tree draws and points the pane at the right draft.
    ///
    /// Replacing the items clears the tree's selection, so the selection is
    /// restored in the same update: the frame after it must not look like a
    /// reader who selected nothing, which would close the open document.
    fn publish(&mut self, cx: &mut Context<DesktopApp>) {
        let items = memory_tree::items(&self.documents);
        let selected = self
            .selected_document()
            .map(|document| SharedString::from(document.path.clone()));
        self.tree.update(cx, |state, cx| {
            state.set_items(items, cx);
            let index = selected.as_ref().and_then(|id| state.index_of(id));
            state.set_selected_index(index, cx);
        });
        self.sync_draft();
        cx.notify();
    }

    /// The documents an open draft touches, which is what the tree marks.
    fn drafted_paths(&self) -> BTreeSet<String> {
        self.documents
            .iter()
            .filter(|document| self.draft_for(document).is_some())
            .map(|document| document.path.clone())
            .collect()
    }

    /// A draft edits a resource, and a document names one, so that pair is what
    /// ties an edit to a file. A draft being created has no resource yet and is
    /// matched by path instead.
    fn draft_for(&self, document: &MemoryDocument) -> Option<&DaemonDraftSummary> {
        self.drafts.iter().find(|draft| {
            draft.target_id.as_deref() == Some(document.resource_id.as_str())
                || draft.path.as_deref() == Some(document.path.as_str())
        })
    }

    /// Points the pane at the open draft that carries the selected document,
    /// which is the one a further edit joins and a Review can name.
    fn sync_draft(&mut self) {
        let draft = self
            .selected_draft()
            .filter(|draft| draft.status == DaemonLocalDraftStatus::Open)
            .cloned();
        self.pane.set_draft(draft);
    }
}
