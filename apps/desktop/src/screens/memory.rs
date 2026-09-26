//! The Memory screen: the Project's file tree beside the documents the reader
//! has open.
//!
//! Read from the macOS client's `MemoryWorkspaceView`: a tree of the Project's
//! Memory beside a strip of open document tabs. macOS keeps one `WorkbenchTab`
//! per document in `WorkspaceNavigation`, with a back stack and a forward stack
//! behind the window's arrows; this screen keeps the same thing, and each open
//! tab holds the editor for its own document, so a tab the reader has left
//! keeps its text, its view mode and its draft.

use std::collections::BTreeSet;

use clumsiesd::{DaemonDraftSummary, DaemonLocalDraftStatus};
use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::tree::TreeState;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::memory_tree;
use crate::engine::{Checkout, DocumentEdit, MemoryDocument};
use crate::screens::document::{DocumentPane, Mode, Notice, PANE_HEADER, PaneContext};
use crate::ui::{self, Typography};

/// One tab. macOS sizes its own between 84 and 200 points; a name that long is
/// rare here, and the truncation is what keeps the strip from walking off.
const TAB_HEIGHT: f32 = 26.;
const TAB_MIN_WIDTH: f32 = 96.;
const TAB_MAX_WIDTH: f32 = 200.;

/// One document the reader has open, with the editor that holds its own work.
struct OpenDocument {
    /// What the tab is: a Memory resource. A resource outlives a re-read of the
    /// Project's checkout, where an index into its documents does not.
    resource_id: String,
    pane: DocumentPane,
}

pub struct MemoryScreen {
    tree: Entity<TreeState>,
    /// The Project the documents belong to, which is what a draft names.
    project_id: Option<String>,
    documents: Vec<MemoryDocument>,
    /// The Project ref the documents resolved from; a new draft is based on it.
    commit_id: Option<String>,
    /// The documents the reader has open, in the order the strip shows them.
    open: Vec<OpenDocument>,
    /// The tab in front: the one the panes draw and the commands act on.
    active: Option<usize>,
    /// Where the reader has been, and where they came back from. The window's
    /// arrows walk these two, which is exactly what macOS's navigation back
    /// and forward stacks are for.
    back: Vec<String>,
    forward: Vec<String>,
    /// A document the reader asked for that this screen has not opened yet.
    ///
    /// A tree click arrives as an entity notification, which carries no window,
    /// and a new tab's editor needs one. The request carries to the next frame,
    /// which has a window.
    pending_open: Option<String>,
    /// The drafts open in this Project, which is what the tree marks and what
    /// the pane offers to review.
    drafts: Vec<DaemonDraftSummary>,
    /// Why the documents could not be read, when they could not be.
    error: Option<String>,
    /// Where the pane's tools take the keyboard. The tools belong to the pane
    /// but the region is the window's (F6 walks it), so the handle is handed
    /// over once instead of being threaded through every call.
    tools_focus: FocusHandle,
    /// Where the file tree takes the keyboard, which is where F6 reaches it.
    /// The tree component keeps a focus handle of its own for clicks; this one
    /// is the screen's, so the window can ask whether the list is the region
    /// the keyboard is in.
    list_focus: FocusHandle,
    /// Dropping a subscription cancels it, so the screen holds it.
    _selection: Subscription,
}

/// What a tree row can offer, read before its menu is built.
pub struct MenuTarget {
    pub draft_id: Option<String>,
    pub can_review: bool,
}

/// Where an arrow key moves the list's selection.
pub enum Move {
    Step(isize),
    First,
    Last,
}

impl MemoryScreen {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
        checkout: Option<Checkout>,
        error: Option<String>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx).items(Vec::new()));
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
            open: Vec::new(),
            active: None,
            back: Vec::new(),
            forward: Vec::new(),
            pending_open: None,
            drafts: Vec::new(),
            error: None,
            tools_focus: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            _selection: selection,
        };
        screen.set_checkout(checkout, error, cx);
        // Opening a Project opens its first document, which is what opening one
        // does in the macOS client, and puts the caret in it: the reader who has
        // just arrived has nothing to browse yet.
        screen.apply_pending_open(window, cx);
        screen.focus_active_editor(window, cx);
        screen
    }

    /// Replaces the documents when the selected Project changes or its checkout
    /// moves.
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
        // A tab belongs to the Project it was opened in: another Project is
        // another set of documents, and macOS drops the tabs the same way when
        // the workspace's authority resets.
        let another_project = self.project_id != project_id;
        if another_project {
            self.open.clear();
            self.active = None;
            self.back.clear();
            self.forward.clear();
        }
        self.project_id = project_id;
        self.commit_id = commit_id;
        self.documents = documents;
        self.error = error;
        self.drafts.clear();
        self.prune();
        if another_project {
            self.pending_open = self
                .documents
                .first()
                .map(|document| document.resource_id.clone());
        }
        self.publish(cx);
    }

    /// Closes the tabs whose document the checkout no longer holds. A checkout
    /// is re-read whenever a session unlocks, so a document that was published
    /// away or never arrived leaves a tab behind, and the tab goes with it.
    fn prune(&mut self) {
        let known: BTreeSet<String> = self
            .documents
            .iter()
            .map(|document| document.resource_id.clone())
            .collect();
        if self.open.iter().all(|tab| known.contains(&tab.resource_id)) {
            return;
        }
        let active = self.active;
        let active_id = self.active_id().map(str::to_owned);
        self.open.retain(|tab| known.contains(&tab.resource_id));
        self.back.retain(|id| known.contains(id));
        self.forward.retain(|id| known.contains(id));
        self.active = match active_id {
            Some(id) => self.tab_index(&id),
            // The tab in front went: the reader keeps the one that took its
            // place, which is what macOS's close does as well.
            None => active.map(|index| index.min(self.open.len().saturating_sub(1))),
        };
        if self.open.is_empty() {
            self.active = None;
        }
    }

    /// The open drafts the daemon holds for this Project. It is asked after
    /// every store, because a store is what creates or advances a draft.
    pub fn set_drafts(&mut self, drafts: Vec<DaemonDraftSummary>, cx: &mut Context<DesktopApp>) {
        self.drafts = drafts;
        self.publish(cx);
    }

    pub fn set_notice(&mut self, notice: Option<Notice>) {
        if let Some(pane) = self.active_pane_mut() {
            pane.set_notice(notice);
        }
    }

    /// The document the tab in front holds, which is what the screen is working
    /// on: the tree marks it, the pane draws it, and an edit belongs to it.
    pub fn selected_document(&self) -> Option<&MemoryDocument> {
        let resource_id = self.active_id()?;
        self.document_for_resource(resource_id)
    }

    /// A document the reader may have open, by the resource it is.
    pub fn document_for_resource(&self, resource_id: &str) -> Option<&MemoryDocument> {
        self.documents
            .iter()
            .find(|document| document.resource_id == resource_id)
    }

    /// The draft carrying the selected document's edits, when it has one.
    pub fn selected_draft(&self) -> Option<&DaemonDraftSummary> {
        self.selected_document()
            .and_then(|document| self.draft_for(document))
    }

    /// The draft carrying a document's edits, whether or not it is in front. A
    /// store that lands belongs to a document, and the reader may have left it.
    pub fn draft_for_resource(&self, resource_id: &str) -> Option<&DaemonDraftSummary> {
        let document = self.document_for_resource(resource_id)?;
        self.draft_for(document)
    }

    pub fn commit_id(&self) -> Option<&str> {
        self.commit_id.as_deref()
    }

    /// The tab in front's editor.
    pub fn active_pane(&self) -> Option<&DocumentPane> {
        let index = self.active?;
        self.open.get(index).map(|tab| &tab.pane)
    }

    pub fn active_pane_mut(&mut self) -> Option<&mut DocumentPane> {
        let index = self.active?;
        self.open.get_mut(index).map(|tab| &mut tab.pane)
    }

    /// The pane holding a document, whether or not it is in front.
    pub fn pane_for_resource(&self, resource_id: &str) -> Option<&DocumentPane> {
        self.open
            .iter()
            .find(|tab| tab.resource_id == resource_id)
            .map(|tab| &tab.pane)
    }

    pub fn pane_for_resource_mut(&mut self, resource_id: &str) -> Option<&mut DocumentPane> {
        self.open
            .iter_mut()
            .find(|tab| tab.resource_id == resource_id)
            .map(|tab| &mut tab.pane)
    }

    /// Which document the editor that just reported a keystroke belongs to.
    /// Every open tab has an editor of its own, so the store follows the tab
    /// the reader typed in rather than the one that happens to be in front.
    pub fn resource_for_editor(&self, editor: EntityId) -> Option<String> {
        self.open
            .iter()
            .find(|tab| tab.pane.editor_id() == editor)
            .map(|tab| tab.resource_id.clone())
    }

    /// Whether there is anywhere to go back to. macOS asks its back stack the
    /// same question, and the window's arrow is drawn from the answer.
    pub fn can_go_back(&self) -> bool {
        self.back.iter().any(|id| self.tab_index(id).is_some())
    }

    pub fn can_go_forward(&self) -> bool {
        self.forward.iter().any(|id| self.tab_index(id).is_some())
    }

    /// Reads the tree's selection into this screen. A folder is not a document,
    /// so selecting one leaves the open tabs alone, which is what macOS does.
    fn follow_selection(&mut self, cx: &App) {
        let path = self
            .tree
            .read(cx)
            .selected_entry()
            .map(|entry| entry.item().id.to_string());
        let Some(resource_id) = path
            .and_then(|path| self.documents.iter().find(|document| document.path == path))
            .map(|document| document.resource_id.clone())
        else {
            return;
        };
        if self.active_id() != Some(resource_id.as_str()) {
            self.pending_open = Some(resource_id);
        }
    }

    /// Opens the document the reader asked for, or brings up the tab it already
    /// has. This runs at the top of a frame, before the panes draw, so a new
    /// tab's editor is already there and already holds its text.
    pub fn apply_pending_open(&mut self, window: &mut Window, cx: &mut Context<DesktopApp>) {
        // A menu built inside the tree talks to the application, so the screen
        // leaves itself where that builder can find it.
        TREE_APP.with(|slot| *slot.borrow_mut() = Some(cx.entity().downgrade()));
        let Some(resource_id) = self.pending_open.take() else {
            return;
        };
        let Some(document) = self
            .documents
            .iter()
            .position(|document| document.resource_id == resource_id)
        else {
            return;
        };
        let index = match self.tab_index(&resource_id) {
            Some(index) => index,
            None => {
                let mut pane = DocumentPane::new(window, cx);
                pane.load(&self.documents[document], window, cx);
                self.open.push(OpenDocument { resource_id, pane });
                self.open.len() - 1
            }
        };
        let typing = self.keyboard_in_editor(window, cx);
        self.activate(index, cx);
        self.follow_editor_focus(typing, window, cx);
    }

    /// Brings up the tab at an index, which is what a click on one does.
    pub fn select_tab(
        &mut self,
        resource_id: &str,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
    ) {
        let Some(index) = self.tab_index(resource_id) else {
            return;
        };
        if self.active == Some(index) {
            self.focus_active_editor(window, cx);
            return;
        }
        self.activate(index, cx);
        self.focus_active_editor(window, cx);
    }

    /// Walks the open documents without touching the history, which is how the
    /// keyboard cycles them.
    pub fn cycle_tab(&mut self, step: isize, window: &mut Window, cx: &mut Context<DesktopApp>) {
        let count = self.open.len();
        let Some(active) = self.active.filter(|_| count > 0) else {
            return;
        };
        let index = (active as isize + step).rem_euclid(count as isize) as usize;
        let typing = self.keyboard_in_editor(window, cx);
        self.activate(index, cx);
        self.follow_editor_focus(typing, window, cx);
    }

    /// Closes one tab. What the reader was writing goes with it: macOS asks
    /// before closing a tab with unsynchronized text, and this client has no
    /// such question yet, so the strip's close button is the whole decision.
    pub fn close_tab(
        &mut self,
        resource_id: &str,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
    ) {
        let Some(index) = self.tab_index(resource_id) else {
            return;
        };
        let active_id = self.active_id().map(str::to_owned);
        let closing_the_active_tab = active_id.as_deref() == Some(resource_id);
        let typing = self.keyboard_in_editor(window, cx);
        self.open.remove(index);
        // A closed tab is not somewhere the arrows can go, which is what macOS
        // does with its back and forward stacks in closeTab.
        self.back.retain(|id| id != resource_id);
        self.forward.retain(|id| id != resource_id);
        if closing_the_active_tab {
            // The tab that took its place, which is the one at the same index
            // until the end of the strip.
            let neighbour = (!self.open.is_empty()).then(|| index.min(self.open.len() - 1));
            self.set_active(neighbour, cx);
            self.follow_editor_focus(typing, window, cx);
            return;
        }
        self.active = active_id.as_deref().and_then(|id| self.tab_index(id));
        self.sync_draft();
        self.select_in_tree(cx);
        cx.notify();
    }

    /// Closes the tab in front, which is what the window's Ctrl+W does.
    pub fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<DesktopApp>) {
        let Some(resource_id) = self.active_id().map(str::to_owned) else {
            return;
        };
        self.close_tab(&resource_id, window, cx);
    }

    /// The document the reader came from.
    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<DesktopApp>) {
        while let Some(resource_id) = self.back.pop() {
            let Some(index) = self.tab_index(&resource_id) else {
                continue;
            };
            if let Some(current) = self.active_id().map(str::to_owned) {
                self.forward.push(current);
            }
            let typing = self.keyboard_in_editor(window, cx);
            self.set_active(Some(index), cx);
            self.follow_editor_focus(typing, window, cx);
            return;
        }
    }

    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<DesktopApp>) {
        while let Some(resource_id) = self.forward.pop() {
            let Some(index) = self.tab_index(&resource_id) else {
                continue;
            };
            if let Some(current) = self.active_id().map(str::to_owned) {
                self.back.push(current);
            }
            let typing = self.keyboard_in_editor(window, cx);
            self.set_active(Some(index), cx);
            self.follow_editor_focus(typing, window, cx);
            return;
        }
    }

    /// The window hands over the handle its F6 cycle uses for the tools region.
    pub fn set_tools_focus(&mut self, focus: FocusHandle) {
        self.tools_focus = focus;
    }

    /// What the tree menu needs to know about one row: whether a draft carries
    /// it, and whether that draft is one a Review could be asked for. A folder
    /// is not a document, so it has nothing to offer yet.
    pub fn menu_target(&self, path: &str) -> Option<MenuTarget> {
        let document = self
            .documents
            .iter()
            .find(|document| document.path == path)?;
        let draft = self.draft_for(document);
        Some(MenuTarget {
            draft_id: draft.map(|draft| draft.draft_id.clone()),
            can_review: draft.is_some_and(|draft| draft.status == DaemonLocalDraftStatus::Open),
        })
    }

    /// What a rename or a deletion is made of: the document, the draft it joins
    /// when it has one, and the base the daemon should record.
    pub fn edit_for_path(&self, path: &str) -> Option<DocumentEdit> {
        let document = self
            .documents
            .iter()
            .find(|document| document.path == path)?;
        let draft = self.draft_for(document);
        Some(DocumentEdit {
            project_id: self.project_id.clone().unwrap_or_default(),
            base_commit_id: draft
                .and_then(|draft| draft.base_commit_id.clone())
                .or_else(|| self.commit_id.clone()),
            draft_id: draft.map(|draft| draft.draft_id.clone()),
            resource_id: document.resource_id.clone(),
            content: document
                .draft_content
                .clone()
                .unwrap_or_else(|| document.content.clone()),
        })
    }

    /// The draft that carries a document's edits, as the daemon names it.
    pub fn draft_for_path(&self, path: &str) -> Option<(String, String)> {
        let document = self
            .documents
            .iter()
            .find(|document| document.path == path)?;
        let draft = self.draft_for(document)?;
        Some((draft.draft_id.clone(), document.resource_id.clone()))
    }

    /// Opens a document for a caller that has a window — a menu, rather than a
    /// tree click, which arrives without one. Returns whether it is open.
    pub fn open_now(
        &mut self,
        path: &str,
        mode: Option<Mode>,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
    ) -> bool {
        let Some(document) = self
            .documents
            .iter()
            .find(|document| document.path == path)
            .cloned()
        else {
            return false;
        };
        let index = match self.tab_index(&document.resource_id) {
            Some(index) => index,
            None => {
                let mut pane = DocumentPane::new(window, cx);
                pane.load(&document, window, cx);
                self.open.push(OpenDocument {
                    resource_id: document.resource_id.clone(),
                    pane,
                });
                self.open.len() - 1
            }
        };
        if let Some(mode) = mode
            && let Some(tab) = self.open.get_mut(index)
        {
            tab.pane.set_mode(mode);
        }
        let typing = self.keyboard_in_editor(window, cx) || mode == Some(Mode::Edit);
        self.activate(index, cx);
        self.follow_editor_focus(typing, window, cx);
        true
    }

    /// Gives the keyboard to the file tree, which is one of the regions F6
    /// walks: a list is reachable by keyboard on this platform, and the tree
    /// component leaves the keys to whoever holds the focus.
    pub fn focus_list(&mut self, window: &mut Window, cx: &mut App) {
        window.focus(&self.list_focus, cx);
    }

    pub fn list_focused(&self, window: &Window) -> bool {
        self.list_focus.is_focused(window)
    }

    /// Moves the tree's selection one row. Selecting a document is what opens
    /// it in macOS as well, so the arrows are how the keyboard reaches the
    /// documents the strip carries tabs for.
    pub fn move_selection(&mut self, movement: Move, cx: &mut Context<DesktopApp>) {
        let rows = self.rows(cx);
        if rows == 0 {
            return;
        }
        let last = rows - 1;
        let current = self.tree.read(cx).selected_index();
        let index = match (current, movement) {
            (_, Move::First) => 0,
            (_, Move::Last) => last,
            (Some(index), Move::Step(step)) => {
                (index as isize + step).clamp(0, last as isize) as usize
            }
            // Nothing is selected: an arrow down starts at the first row, and
            // an arrow up at the last.
            (None, Move::Step(step)) => {
                if step < 0 {
                    last
                } else {
                    0
                }
            }
        };
        if current == Some(index) {
            return;
        }
        self.tree.update(cx, |state, cx| {
            state.set_selected_index(Some(index), cx);
            state.scroll_to_item(index, ScrollStrategy::Nearest);
        });
        cx.notify();
    }

    /// How many rows the tree is showing. The component keeps its flattened
    /// list to itself, so asking it row by row until it stops answering is the
    /// count there is; a Project's Memory is small enough that walking it is
    /// cheaper than keeping a second copy of the same walk.
    fn rows(&self, cx: &App) -> usize {
        let state = self.tree.read(cx);
        (0..)
            .take_while(|index| state.entry(*index).is_some())
            .count()
    }

    /// What the pane needs to name the document in front.
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
    pub fn list(
        &self,
        project: AnyElement,
        settings: Option<AnyElement>,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        // No title: the rail already says which section this is, and a heading
        // that repeats it is a line of pixels that says nothing. The filter is
        // what a reader needs here; the right side is for the one command that
        // is rarer than the work, and stays empty otherwise.
        let header = div()
            .h_flex()
            .h(px(PANE_HEADER))
            .pl_3()
            .pr_2()
            .gap_2()
            .items_center()
            .child(project)
            .child(div().flex_1().min_w(px(0.)))
            .children(settings);
        // The tree's region takes the keyboard as one thing, and says so with a
        // ring, the way every other focusable region in this window does.
        let ring = if self.list_focused(window) {
            cx.theme().ring
        } else {
            transparent_black()
        };

        div()
            .v_flex()
            .h_full()
            .child(header)
            .child(ui::rule(cx))
            .child(
                div()
                    .id("memory-tree")
                    .flex_1()
                    .min_h(px(0.))
                    .m_2()
                    .p_1()
                    .rounded(px(ui::RADIUS))
                    .border_1()
                    .border_color(ring)
                    .track_focus(&self.list_focus)
                    .tab_stop(true)
                    .child(memory_tree::memory_tree(
                        &self.tree,
                        &self.drafted_paths(),
                        |path, menu, window, cx| tree_menu(path, menu, window, cx),
                    )),
            )
            .into_any_element()
    }

    /// The section's detail: the strip of open documents over the tab in front,
    /// which draws under the pane's own header.
    pub fn detail(&self, focus: &FocusHandle, cx: &mut Context<DesktopApp>) -> AnyElement {
        let Some(pane) = self.active_pane() else {
            // Three ways to have nothing to show: a Project that could not be
            // read, one with no Memory at all, and one whose tabs the reader
            // has all closed.
            let reason = match (&self.error, self.documents.is_empty()) {
                (Some(error), _) => ui::message(error.clone(), cx.theme().danger),
                (None, true) => ui::message(
                    "This Project has no Memory yet.",
                    cx.theme().muted_foreground,
                ),
                (None, false) => ui::message("Select a document.", cx.theme().muted_foreground),
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
        // One toolbar for the pane: which document is open on the left, what
        // can be done with it on the right. macOS splits the same two between
        // its tab strip and its window toolbar.
        let toolbar = div()
            .h_flex()
            .h(px(PANE_HEADER))
            .pl_2()
            .pr_3()
            .gap_2()
            .items_center()
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_1()
                    .items_center()
                    .overflow_hidden()
                    .children(self.tabs(cx)),
            )
            .child(pane.tools(focus, cx));
        let body = pane.body(cx);
        div()
            .v_flex()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(toolbar)
            .child(ui::rule(cx))
            .child(body)
            .into_any_element()
    }

    /// The open documents as tabs, which are the left half of the pane's
    /// toolbar: macOS's DocumentTabStrip, at this platform's metrics. One tab
    /// per document, the one in front drawn as a surface of its own, and a
    /// close button on every tab.
    fn tabs(&self, cx: &mut Context<DesktopApp>) -> Vec<AnyElement> {
        if self.open.is_empty() {
            return Vec::new();
        }
        // The colors a chip needs are taken once: a listener borrows the
        // application, so the theme cannot be held across one.
        let (surface, border, foreground, muted, hover) = {
            let theme = cx.theme();
            (
                theme.background,
                theme.border,
                theme.foreground,
                theme.muted_foreground,
                theme.secondary_hover,
            )
        };
        let active = self.active;
        let mut chips: Vec<AnyElement> = Vec::with_capacity(self.open.len());
        for (index, tab) in self.open.iter().enumerate() {
            let selected = active == Some(index);
            let opening = tab.resource_id.clone();
            let closing = tab.resource_id.clone();
            let title = self
                .document_for_resource(&tab.resource_id)
                .map(|document| file_name(&document.path))
                .unwrap_or_else(|| ui::shorten(&tab.resource_id, 12));
            // The tool a tab is using is worth saying, because a tab the reader
            // is not looking at cannot show its tools. Reading is the default
            // and stays unlabelled: macOS appends it to every tab, which is
            // noise when it is what documents open as.
            let label = match tab.pane.mode() {
                Mode::Edit => format!("{title} — editing"),
                Mode::Diff => format!("{title} — diff"),
                Mode::Preview => title,
            };
            let tone = if selected { foreground } else { muted };
            let mut chip =
                div()
                    .id(("document-tab", index))
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .h(px(TAB_HEIGHT))
                    .min_w(px(TAB_MIN_WIDTH))
                    .max_w(px(TAB_MAX_WIDTH))
                    .flex_shrink_0()
                    .pl_2()
                    .pr_1()
                    .rounded(px(ui::RADIUS))
                    .text_color(tone)
                    .on_click(cx.listener(move |app, _event, window, cx| {
                        app.select_tab(&opening, window, cx)
                    }));
            if selected {
                chip = chip.bg(surface).border_1().border_color(border);
            } else {
                chip = chip.hover(|style| style.bg(hover));
            }
            chips.push(
                chip.child(
                    Icon::new(IconName::FileText)
                        .with_size(px(12.))
                        .flex_shrink_0()
                        .text_color(tone),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_style(&ui::CAPTION)
                        .child(label),
                )
                .child(
                    div()
                        .id(("close-document-tab", index))
                        .h_flex()
                        .justify_center()
                        .items_center()
                        .size(px(18.))
                        .flex_shrink_0()
                        .rounded(px(ui::RADIUS))
                        .hover(|style| style.bg(hover))
                        .on_click(cx.listener(move |app, _event, window, cx| {
                            app.close_tab(&closing, window, cx)
                        }))
                        .child(
                            Icon::new(IconName::Close)
                                .with_size(px(12.))
                                .text_color(muted),
                        ),
                )
                .into_any_element(),
            );
        }
        chips
    }

    /// Rebuilds what the tree draws and points the panes at the right draft.
    ///
    /// Replacing the items clears the tree's selection, so the selection is
    /// restored in the same update: the frame after it must not look like a
    /// reader who selected nothing, which would close the open document.
    fn publish(&mut self, cx: &mut Context<DesktopApp>) {
        let items = memory_tree::items(&self.documents);
        self.tree.update(cx, |state, cx| state.set_items(items, cx));
        self.select_in_tree(cx);
        self.sync_draft();
        cx.notify();
    }

    /// Points the tree at the document in front, without touching what it
    /// draws. The tree's own notification reads this back as the same document,
    /// which is why it cannot open a second tab for it.
    fn select_in_tree(&mut self, cx: &mut Context<DesktopApp>) {
        let selected = self
            .selected_document()
            .map(|document| SharedString::from(document.path.clone()));
        self.tree.update(cx, |state, cx| {
            let index = selected.as_ref().and_then(|id| state.index_of(id));
            state.set_selected_index(index, cx);
        });
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

    /// Points the tab in front at the open draft that carries its document,
    /// which is the one a further edit joins and a Review can name.
    fn sync_draft(&mut self) {
        let draft = self
            .selected_draft()
            .filter(|draft| draft.status == DaemonLocalDraftStatus::Open)
            .cloned();
        if let Some(pane) = self.active_pane_mut() {
            pane.set_draft(draft);
        }
    }

    /// The resource of the tab in front.
    fn active_id(&self) -> Option<&str> {
        let index = self.active?;
        self.open.get(index).map(|tab| tab.resource_id.as_str())
    }

    fn tab_index(&self, resource_id: &str) -> Option<usize> {
        self.open
            .iter()
            .position(|tab| tab.resource_id == resource_id)
    }

    /// Brings up a tab, which is what opening, clicking and the history all end
    /// in. Where the reader came from joins the back stack and the forward
    /// stack empties, which is macOS's rule for the same act.
    fn activate(&mut self, index: usize, cx: &mut Context<DesktopApp>) {
        let arriving = self.open[index].resource_id.clone();
        if let Some(previous) = self.active_id().map(str::to_owned)
            && previous != arriving
        {
            self.back.push(previous);
            self.forward.clear();
        }
        self.set_active(Some(index), cx);
    }

    /// Makes one tab the tab in front, without touching the history. Closing a
    /// tab and the history arrows both land here.
    fn set_active(&mut self, index: Option<usize>, cx: &mut Context<DesktopApp>) {
        self.active = index;
        self.sync_draft();
        self.select_in_tree(cx);
        cx.notify();
    }

    /// Whether the keyboard is in the tab in front's text.
    fn keyboard_in_editor(&self, window: &Window, cx: &App) -> bool {
        self.active_pane()
            .is_some_and(|pane| pane.editor_focused(window, cx))
    }

    /// Puts the keyboard back in the text when that is where it was before the
    /// tab in front changed.
    fn follow_editor_focus(&self, typing: bool, window: &mut Window, cx: &mut App) {
        if typing {
            self.focus_active_editor(window, cx);
        }
    }

    /// Puts the keyboard where the open document can be read or written. A
    /// document opens to be read, and the editor exists only while it is being
    /// edited, so the pane's tools take the keyboard in every other mode — a
    /// window with nothing focused is a window that drops every key.
    pub fn focus_open_document(&self, window: &mut Window, cx: &mut App) {
        self.focus_active_editor(window, cx);
    }

    fn focus_active_editor(&self, window: &mut Window, cx: &mut App) {
        match self.active_pane() {
            Some(pane) if pane.mode() == Mode::Edit => pane.focus_editor(window, cx),
            _ => window.focus(&self.tools_focus, cx),
        }
    }
}

/// What the tree offers for one row, which is what the Project's drafts say
/// about that document. macOS puts the same commands in its row context menu;
/// the menu itself is the component library's, so arrows move, Enter chooses
/// and Escape closes the way they do everywhere else on this platform.
fn tree_menu(path: &str, menu: PopupMenu, _window: &mut Window, cx: &mut App) -> PopupMenu {
    let Some(this) = TREE_APP.with(|slot| slot.borrow().as_ref().and_then(|weak| weak.upgrade()))
    else {
        return menu;
    };
    let Some(target) = this.read_with(cx, |app, _| app.memory_ref().menu_target(path)) else {
        return menu;
    };
    let opening = this.clone();
    let editing = this.clone();
    let reviewing = this.clone();
    let discarding = this.clone();
    let opening_path = path.to_owned();
    let editing_path = path.to_owned();
    let reviewing_path = path.to_owned();
    let discarding_path = path.to_owned();
    let mut menu = menu
        .item(
            PopupMenuItem::new("Open").on_click(move |_event, window, cx| {
                opening.update(cx, |app, cx| {
                    app.open_document(&opening_path, Mode::Preview, window, cx)
                });
            }),
        )
        .item(
            PopupMenuItem::new("Edit").on_click(move |_event, window, cx| {
                editing.update(cx, |app, cx| {
                    app.open_document(&editing_path, Mode::Edit, window, cx)
                });
            }),
        );
    // The generic file commands, which any tree offers, then Memory's own: macOS
    // splits its own row menu the same way, and the two sections do not mix.
    let renaming = this.clone();
    let deleting = this.clone();
    let rename_path = path.to_owned();
    let delete_path = path.to_owned();
    menu = menu
        .separator()
        .item(
            PopupMenuItem::new("Rename…").on_click(move |_event, window, cx| {
                renaming.update(cx, |app, cx| {
                    app.open_rename_dialog(&rename_path, window, cx)
                });
            }),
        )
        .item(
            PopupMenuItem::new("Delete…").on_click(move |_event, window, cx| {
                deleting.update(cx, |app, cx| {
                    app.open_delete_dialog(&delete_path, window, cx)
                });
            }),
        );
    if target.can_review {
        menu = menu
            .separator()
            .item(
                PopupMenuItem::new("Request review…").on_click(move |_event, window, cx| {
                    reviewing.update(cx, |app, cx| {
                        app.request_review_for(&reviewing_path, window, cx)
                    });
                }),
            );
    }
    if target.draft_id.is_some() {
        menu = menu.item(PopupMenuItem::new("Discard draft").on_click(
            move |_event, _window, cx| {
                discarding.update(cx, |app, cx| app.discard_draft_for(&discarding_path, cx));
            },
        ));
    }
    menu
}

// The entity a menu built inside a component can reach. A menu builder runs
// with the application rather than with the screen that drew the tree, so the
// screen leaves itself here while it renders.
thread_local! {
    static TREE_APP: std::cell::RefCell<Option<WeakEntity<DesktopApp>>> =
        const { std::cell::RefCell::new(None) };
}

/// The last segment of a path, which is what a tab calls a document.
fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}
