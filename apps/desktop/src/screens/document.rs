//! The document pane: one Memory document, its editor, and the draft flow that
//! carries an edit to a Review.
//!
//! Read from the macOS client's `DocumentSessionView`, `DocumentEditorModel`
//! and `ReviewRequestSheet`: the same three modes (source, preview, and a diff
//! against what the checkout holds), the same autosave after a pause in typing,
//! and the same rule that a Review is requested from the document the edit
//! belongs to. Where this client differs, the difference is stated where it
//! happens rather than left for a reader to find.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use clumsiesd::DaemonDraftSummary;
use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::{diff, fill, markdown};
use crate::engine::{self, DocumentEdit, MemoryDocument};
use crate::ui::{self, Typography};

/// How long a pause in typing waits before the text is stored. macOS debounces
/// at 600ms because the store is a socket call, not an in-process write.
pub const SAVE_DELAY: Duration = Duration::from_millis(600);

/// The height of a pane's header row. The list column and the document pane use
/// the same one, so the two headers line up across the card.
pub const PANE_HEADER: f32 = 44.;

/// The three ways a document can be read, which are the macOS tab modes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Source,
    Preview,
    /// What this edit changes against the version the checkout holds.
    Diff,
}

impl Mode {
    fn index(self) -> usize {
        match self {
            Mode::Source => 0,
            Mode::Preview => 1,
            Mode::Diff => 2,
        }
    }

    fn from_index(index: usize) -> Self {
        match index {
            1 => Mode::Preview,
            2 => Mode::Diff,
            _ => Mode::Source,
        }
    }
}

/// What the engine has done with the text in the editor.
pub enum SaveState {
    /// The editor holds what the engine last accepted.
    Clean,
    /// Keystrokes are waiting for the pause that stores them.
    Pending,
    /// A store is on its way to the engine.
    Saving,
    /// The engine accepted the text.
    Saved,
    Failed(String),
}

/// A line about something that already happened, such as the Review a request
/// produced. A failure is not one of these: it belongs to the state it failed,
/// which is why the pane has one failure channel and it is the save.
pub struct Notice {
    pub text: String,
}

/// What the pane needs from the screen it sits in: which Project the document
/// belongs to, and the document itself.
pub struct PaneContext<'a> {
    pub project_id: &'a str,
    /// The Project ref the checkout resolved to, which bases a new draft.
    pub commit_id: Option<&'a str>,
    pub document: &'a MemoryDocument,
}

pub struct DocumentPane {
    editor: Entity<TextareaState>,
    mode: Mode,
    /// The text the checkout resolved to when the document was opened. A diff
    /// is measured against it, because that is what a reviewer would see.
    base_text: String,
    /// The text the engine has accepted, which is what "unsaved" is measured
    /// against.
    saved_text: String,
    /// What the engine has done with the text in the editor.
    save: SaveState,
    /// The debounced store this pane's text belongs to, so that a store which
    /// lands after a later keystroke is not reported as the current state.
    generation: u64,
    /// How tall the editor's box turned out, which is what the editor is told:
    /// it cannot ask for a percentage inside a column sized by flex, so the
    /// pane measures the box and hands the number back. See `components::fill`.
    editor_height: Rc<Cell<Pixels>>,
    /// The open draft carrying this document's edits, when it has one.
    draft: Option<DaemonDraftSummary>,
    notice: Option<Notice>,
    review_title: Entity<InputState>,
    review_description: Entity<TextareaState>,
    /// Dropping a subscription cancels it, so the pane holds it.
    _edits: Subscription,
}

impl DocumentPane {
    pub fn new(window: &mut Window, cx: &mut Context<DesktopApp>) -> Self {
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write what this Project should remember.")
                // A document wraps. The component wraps by default, and this
                // says so where the editor is made: a horizontal scrollbar in
                // the middle of prose is how a reader loses their place.
                .soft_wrap(true)
        });
        let review_title =
            cx.new(|cx| InputState::new(window, cx).placeholder("What this change does"));
        let review_description = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("Why, and anything a reviewer should check.")
        });
        // The editor reports every keystroke; the application decides when a
        // pause is long enough to store the text.
        let edits = cx.subscribe(&editor, |app, editor, event, cx| {
            if matches!(event, InputEvent::Change) {
                app.document_edited(editor.entity_id(), cx);
            }
        });
        Self {
            editor,
            mode: Mode::Source,
            base_text: String::new(),
            saved_text: String::new(),
            save: SaveState::Clean,
            generation: 0,
            editor_height: Rc::new(Cell::new(px(0.))),
            draft: None,
            notice: None,
            review_title,
            review_description,
            _edits: edits,
        }
    }

    /// Opens a document: its text becomes the editor's, and the diff and the
    /// Review title start from what the checkout holds. Setting the text does
    /// not report a change, so opening a document cannot store anything.
    pub fn load(
        &mut self,
        document: &MemoryDocument,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
    ) {
        // The proposal when there is one, because that is what the reader last
        // wrote; the published text is what the diff measures against.
        self.base_text = document.content.clone();
        let text = document
            .draft_content
            .clone()
            .unwrap_or_else(|| document.content.clone());
        self.saved_text = text.clone();
        self.save = SaveState::Clean;
        self.draft = None;
        self.notice = None;
        let title = document_title(&text, &document.path);
        self.review_title
            .update(cx, |state, cx| state.set_value(title, window, cx));
        self.editor
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }

    /// Puts the caret in the editor, which is where a reader who has just
    /// opened a document types next. macOS does the same when a document
    /// becomes the active tab.
    pub fn focus_editor(&self, window: &mut Window, cx: &mut App) {
        let handle = self.editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Whether the keyboard is in this pane's text. Which region holds it is
    /// what decides whether changing the tab in front moves it: a reader
    /// arrowing through the tree is browsing and stays there, while a reader
    /// typing follows the text to the tab that took its place.
    pub fn editor_focused(&self, window: &Window, cx: &App) -> bool {
        self.editor.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    /// Whether the editor holds text the engine has not accepted yet.
    pub fn dirty(&self, cx: &App) -> bool {
        self.text(cx) != self.saved_text
    }

    /// The engine accepted this text, which is what "unsaved" is measured
    /// against from here on.
    pub fn accept_text(&mut self, text: String) {
        self.saved_text = text;
    }

    pub fn set_draft(&mut self, draft: Option<DaemonDraftSummary>) {
        self.draft = draft;
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The editor this pane owns, which is how a keystroke finds the tab it was
    /// typed in rather than the tab that happens to be in front.
    pub fn editor_id(&self) -> EntityId {
        self.editor.entity_id()
    }

    /// Which store owns this pane's text. A screen with several panes open
    /// counts stores per document, so a store in one tab cannot report itself
    /// as another tab's state.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn set_generation(&mut self, generation: u64) {
        self.generation = generation;
    }

    pub fn set_notice(&mut self, notice: Option<Notice>) {
        self.notice = notice;
    }

    pub fn set_save_state(&mut self, save: SaveState) {
        self.save = save;
    }

    /// The document pane's own header: which document is open, how to look at
    /// it, what it has to say about itself, and what the screen can do with it.
    /// macOS keeps the first two in the window toolbar; here they sit on the
    /// pane they act on, above the text.
    pub fn header(&self, actions: Option<AnyElement>, cx: &mut Context<DesktopApp>) -> AnyElement {
        let this = cx.entity();
        let modes = TabBar::new("document-mode")
            .segmented()
            .selected_index(self.mode.index())
            .on_click({
                let this = this.clone();
                move |index, _window, cx| {
                    let mode = Mode::from_index(*index);
                    this.update(cx, |app, cx| app.set_document_mode(mode, cx));
                }
            })
            .children([
                Tab::new().label("Source"),
                Tab::new().label("Preview"),
                Tab::new().label("Diff"),
            ]);
        div()
            .h_flex()
            .h(px(PANE_HEADER))
            .px_4()
            .gap_3()
            .items_center()
            .child(modes)
            .children(self.header_status(cx))
            .children(self.header_notice(cx))
            .child(div().flex_1().min_w(px(0.)))
            .children(actions)
            .into_any_element()
    }

    /// The work itself: the document, read in one of its three modes, under the
    /// pane's header.
    pub fn detail(&self, actions: Option<AnyElement>, cx: &mut Context<DesktopApp>) -> AnyElement {
        let text = self.text(cx);

        let body: AnyElement = match self.mode {
            // The pane is the surface, so the field draws no box of its own:
            // the document is the whole of what a reader sees here. Its height
            // is the box the pane measured, because the editor's own element
            // asks for a percentage, which nothing in a flex column resolves.
            Mode::Source => {
                let height = self.editor_height.clone();
                let this = cx.entity().downgrade();
                let editor = Textarea::new(&self.editor)
                    .appearance(false)
                    .bordered(false)
                    .h(height.get().max(px(1.)));
                fill::Fill::new(height)
                    .on_measure(move |cx| {
                        this.update(cx, |_app, cx| cx.notify()).ok();
                    })
                    .child(editor)
                    .into_any_element()
            }
            Mode::Preview => div()
                .flex_1()
                .min_h(px(0.))
                .child(markdown::memory_document("document-preview", text.clone()))
                .into_any_element(),
            Mode::Diff => {
                let rows = diff::diff_rows(&self.base_text, &text);
                if rows.iter().all(|row| row.kind == diff::DiffKind::Context) {
                    div()
                        .flex_1()
                        .child(ui::message(
                            "This document matches the Project's checkout.",
                            cx.theme().muted_foreground,
                        ))
                        .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .child(diff::diff_view(
                            rows,
                            cx.theme().mono_font_family.clone(),
                            diff::DiffPalette::from_theme(cx.theme()),
                        ))
                        .into_any_element()
                }
            }
        };

        div()
            .v_flex()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(self.header(actions, cx))
            .child(ui::rule(cx))
            // The body is a flex column of its own: a percentage or a flex
            // share only reaches a control whose parent lays out as flex, and
            // the editor needs one of the two.
            .child(div().v_flex().flex_1().min_h(px(0.)).p_4().child(body))
            .into_any_element()
    }

    /// What the pane's header says beyond the document's name: only what a
    /// reader has to act on. A document that is saved says nothing — the
    /// version the Server holds is not news — while a document that is being
    /// written, or that failed to write, says so where the eye already is.
    pub fn header_status(&self, cx: &App) -> Option<AnyElement> {
        let (text, color) = match &self.save {
            SaveState::Pending => ("Unsaved changes".to_owned(), cx.theme().muted_foreground),
            SaveState::Saving => ("Saving…".to_owned(), cx.theme().muted_foreground),
            SaveState::Failed(error) => (error.clone(), cx.theme().danger),
            SaveState::Clean | SaveState::Saved => return None,
        };
        Some(
            div()
                .text_style(&ui::CAPTION)
                .text_color(color)
                .child(text)
                .into_any_element(),
        )
    }

    /// What a Review answered, which the pane keeps until the document it was
    /// asked for changes.
    pub fn header_notice(&self, cx: &App) -> Option<AnyElement> {
        let notice = self.notice.as_ref()?;
        Some(
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().success)
                .child(notice.text.clone())
                .into_any_element(),
        )
    }

    /// The edit this pane would hand the engine for a Review, and whether its
    /// text still has to be stored first. Both the header's action and the
    /// keyboard action go through here, so they cannot disagree.
    pub fn review_edit(&self, target: &PaneContext<'_>, cx: &App) -> (DocumentEdit, bool) {
        let edit = DocumentEdit {
            project_id: target.project_id.to_owned(),
            base_commit_id: self
                .draft
                .as_ref()
                .and_then(|draft| draft.base_commit_id.clone())
                .or_else(|| target.commit_id.map(str::to_owned)),
            draft_id: self.draft.as_ref().map(|draft| draft.draft_id.clone()),
            resource_id: target.document.resource_id.clone(),
            content: self.text(cx),
        };
        (edit, self.dirty(cx))
    }

    pub fn review_title(&self) -> Entity<InputState> {
        self.review_title.clone()
    }

    pub fn review_description(&self) -> Entity<TextareaState> {
        self.review_description.clone()
    }

    /// Whether a Review can be asked for this document at all.
    pub fn can_review(&self) -> bool {
        self.draft.is_some()
    }
}

/// Opens the sheet that requests a Review for one stored edit.
///
/// The request runs in the sheet, so the sheet is the thing that knows whether
/// it is waiting on the network; this only puts it on screen.
pub(crate) fn open_review_sheet(
    edit: DocumentEdit,
    store: bool,
    title: Entity<InputState>,
    description: Entity<TextareaState>,
    app: WeakEntity<DesktopApp>,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| ReviewDialog::new(cx, title, description, edit, store, app));
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let view = view.clone();
        dialog
            .title("Request review")
            .w(px(520.))
            .keyboard(true)
            .content(move |content, _window, _cx| content.child(view.clone()))
            .footer(div())
            .footer(div())
    });
}

/// The title a Review starts from: the document's own frontmatter title when it
/// has one, and its path when it does not. macOS reads the same field from its
/// catalog.
fn document_title(content: &str, path: &str) -> String {
    let mut lines = content.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return path.to_owned();
    }
    for line in lines {
        let line = line.trim_end();
        if line == "---" {
            break;
        }
        if let Some(title) = line.strip_prefix("title:") {
            let title = title.trim().trim_matches(['"', '\'']);
            if !title.is_empty() {
                return title.to_owned();
            }
        }
    }
    path.to_owned()
}

/// The sheet that requests a Review for one draft.
///
/// It owns the request rather than the pane, so a failure is shown in the sheet
/// that produced it — which is what the macOS sheet does, and what Windows asks
/// for when it says an error belongs where the input is. The fields belong to
/// the pane, so dismissing the sheet keeps what was typed.
struct ReviewDialog {
    title: Entity<InputState>,
    description: Entity<TextareaState>,
    /// The edit this Review is requested for, captured when the sheet opened.
    edit: DocumentEdit,
    /// Whether the editor's text still has to be stored before the request.
    store: bool,
    busy: bool,
    error: Option<String>,
    app: WeakEntity<DesktopApp>,
    /// Dropping a subscription cancels it, so the sheet holds them.
    _title_edits: Subscription,
    _description_edits: Subscription,
}

impl ReviewDialog {
    fn new(
        cx: &mut Context<Self>,
        title: Entity<InputState>,
        description: Entity<TextareaState>,
        edit: DocumentEdit,
        store: bool,
        app: WeakEntity<DesktopApp>,
    ) -> Self {
        // The Request button lives or dies by whether the title says something,
        // so the sheet redraws when either field changes.
        let title_edits = cx.observe(&title, |_, _, cx| cx.notify());
        let description_edits = cx.observe(&description, |_, _, cx| cx.notify());
        Self {
            title,
            description,
            edit,
            store,
            busy: false,
            error: None,
            app,
            _title_edits: title_edits,
            _description_edits: description_edits,
        }
    }

    /// A Review needs a title, which is the rule the macOS sheet enforces by
    /// disabling its own button.
    fn ready(&self, cx: &App) -> bool {
        !self.busy && !self.title.read(cx).value().trim().is_empty()
    }

    /// Stores the edit if it is still pending, waits for the daemon to upload
    /// the draft, and asks the Server for a Review — one background job, so the
    /// window keeps drawing while the network works.
    fn request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        let title = self.title.read(cx).value().trim().to_owned();
        let description = self.description.read(cx).value().trim().to_owned();
        self.busy = true;
        self.error = None;
        cx.notify();

        let edit = self.edit.clone();
        let store = self.store;
        let app = self.app.clone();
        let work = cx.background_executor().spawn(async move {
            engine::submit_document_review(&edit, store, &title, &description)
        });
        // Spawned in the window rather than the application: closing the sheet
        // needs the window, and a closed sheet is what a successful request
        // looks like.
        cx.spawn_in(window, async move |this, cx| match work.await {
            Ok(review) => {
                cx.update(|window, cx| {
                    window.close_dialog(cx);
                    app.update(cx, |app, cx| app.review_requested(review, cx))
                        .ok();
                })
                .ok();
            }
            Err(error) => {
                this.update(cx, |dialog, cx| {
                    dialog.busy = false;
                    dialog.error = Some(error);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}

impl Render for ReviewDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.busy {
            "Requesting review…"
        } else {
            "Request review"
        };
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(label_line("Title *", cx))
                    .child(Input::new(&self.title)),
            )
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(label_line("Description", cx))
                    .child(Textarea::new(&self.description).h(px(96.))),
            )
            .children(self.error.as_ref().map(|error| {
                div()
                    .text_style(&ui::BODY)
                    .text_color(cx.theme().danger)
                    .child(error.clone())
            }))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("request")
                            .primary()
                            .label(label)
                            .disabled(!self.ready(cx))
                            .on_click(
                                cx.listener(|dialog, _, window, cx| dialog.request(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .disabled(self.busy)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_style(&ui::CAPTION)
                            .text_color(cx.theme().muted_foreground)
                            .child("Escape cancels."),
                    ),
            )
    }
}

fn label_line(label: &str, cx: &App) -> impl IntoElement {
    div()
        .text_style(&ui::CAPTION)
        .text_color(cx.theme().muted_foreground)
        .child(label.to_owned())
}
