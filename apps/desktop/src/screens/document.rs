//! The document pane: one Memory document, its editor, and the draft flow that
//! carries an edit to a Review.
//!
//! Read from the macOS client's `DocumentSessionView`, `DocumentEditorModel`
//! and `ReviewRequestSheet`: the same three modes (source, preview, and a diff
//! against what the checkout holds), the same autosave after a pause in typing,
//! and the same rule that a Review is requested from the document the edit
//! belongs to. Where this client differs, the difference is stated where it
//! happens rather than left for a reader to find.

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
use crate::components::{diff, markdown};
use crate::engine::{self, DocumentEdit, MemoryDocument};
use crate::ui::{self, Typography};

/// How long a pause in typing waits before the text is stored. macOS debounces
/// at 600ms because the store is a socket call, not an in-process write.
pub const SAVE_DELAY: Duration = Duration::from_millis(600);

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
            TextareaState::new(window, cx).placeholder("Write what this Project should remember.")
        });
        let review_title =
            cx.new(|cx| InputState::new(window, cx).placeholder("What this change does"));
        let review_description = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("Why, and anything a reviewer should check.")
        });
        // The editor reports every keystroke; the application decides when a
        // pause is long enough to store the text.
        let edits = cx.subscribe(&editor, |app, _editor, event, cx| {
            if matches!(event, InputEvent::Change) {
                app.document_edited(cx);
            }
        });
        Self {
            editor,
            mode: Mode::Source,
            base_text: String::new(),
            saved_text: String::new(),
            save: SaveState::Clean,
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

    pub fn set_notice(&mut self, notice: Option<Notice>) {
        self.notice = notice;
    }

    pub fn set_save_state(&mut self, save: SaveState) {
        self.save = save;
    }

    /// Whether the draft this document edits has reached the Server, which is
    /// what a Review needs.
    fn uploaded(&self) -> bool {
        self.draft.as_ref().is_some_and(|draft| {
            draft.server_draft_id.is_some()
                && draft.pending_operation_count == 0
                && draft.failed_operation_count == 0
        })
    }

    /// One line about the last edit, in the color its meaning asks for.
    fn save_line(&self, cx: &App) -> (String, Hsla) {
        if let SaveState::Failed(error) = &self.save {
            return (error.clone(), cx.theme().danger);
        }
        let draft = match &self.draft {
            Some(draft) if self.uploaded() => {
                Some(format!("draft v{} on the Server", draft.server_version))
            }
            Some(_) => Some("draft uploading".to_owned()),
            None => None,
        };
        let text = match (&self.save, &draft) {
            (SaveState::Pending, _) => "Unsaved changes".to_owned(),
            (SaveState::Saving, _) => "Saving…".to_owned(),
            (SaveState::Saved, Some(draft)) => format!("Saved · {draft}"),
            (SaveState::Saved, None) => "Saved".to_owned(),
            (SaveState::Clean, Some(draft)) => {
                let mut line = draft.clone();
                line[..1].make_ascii_uppercase();
                line
            }
            (SaveState::Clean, None) => "No local changes".to_owned(),
            (SaveState::Failed(error), _) => error.clone(),
        };
        (text, cx.theme().muted_foreground)
    }

    /// The work itself: the document, read in one of its three modes. The
    /// window's actions live in the shell's context bar, so this pane is only
    /// ever the document and how to look at it.
    pub fn detail(
        &self,
        target: Option<PaneContext<'_>>,
        cx: &mut Context<DesktopApp>,
    ) -> AnyElement {
        let Some(target) = target else {
            return div()
                .v_flex()
                .flex_1()
                .h_full()
                .p_4()
                .child(ui::message(
                    "Select a document.",
                    cx.theme().muted_foreground,
                ))
                .into_any_element();
        };
        let text = self.text(cx);
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

        let header = div()
            .h_flex()
            .gap_3()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_style(&ui::BODY)
                    .child(target.document.path.clone()),
            )
            .child(modes);

        let body: AnyElement = match self.mode {
            Mode::Source => div()
                .flex_1()
                .min_h(px(0.))
                .child(Textarea::new(&self.editor).h(relative(1.)))
                .into_any_element(),
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
            .h_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .p_4()
            .gap_3()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The document's line in the window's status bar: what the engine has done
    /// with the last edit, and what a Review answered.
    pub fn status(&self, cx: &App) -> AnyElement {
        let (text, color) = self.save_line(cx);
        div()
            .h_flex()
            .gap_3()
            .items_center()
            .child(div().text_style(&ui::CAPTION).text_color(color).child(text))
            .children(self.notice.as_ref().map(|notice| {
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().success)
                    .child(notice.text.clone())
            }))
            .into_any_element()
    }

    /// The edit this pane would hand the engine for a Review, and whether its
    /// text still has to be stored first. Both the context bar's action and the
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
