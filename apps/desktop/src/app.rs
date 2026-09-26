//! The desktop client's window: the shell, and the flows that fill it.
//!
//! The window's shape lives in shell.rs; this file owns what the flows are and
//! what they do to the engine. The macOS client draws the same split: its
//! WorkspaceView composes the shell, and the models behind each section hold the
//! work.

use clumsiesd::{DaemonDraftOperationResponse, DaemonDraftSummary};
use gpui_kit::base::Disableable;
use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::*;
use gpui_kit::component::{Icon, Root, Theme, WindowExt as _};
use gpui_kit::*;

use crate::engine::{self, Checkout, DocumentEdit, EngineStatus, Project, Review, ReviewStatus};
use crate::screens::dialogs::{ConfirmDialog, DialogAction, RenameDialog, RenameFolderDialog};
use crate::screens::document::{self, Mode, Notice, SAVE_DELAY, SaveState};
use crate::screens::guidelines;
use crate::screens::memory::{MemoryScreen, Move};
use crate::screens::new_memory::NewMemoryDialog;
use crate::screens::project_settings::{ProjectSettings, ProjectSettingsDialog};
use crate::screens::reviews::{ReviewNotice, ReviewsScreen};
use crate::screens::sign_in::{SignInScreen, StagedSetup};
use crate::shell::{Chrome, EngineFacts, Section, Shell, Slots};
use crate::ui::{self, Typography};

/// Which region of the window the keyboard is in. F6 walks these in this
/// order, which is the order the platform's Tab would visit them in if a
/// document editor did not consume Tab.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Rail,
    List,
    Detail,
    Actions,
}

pub struct DesktopApp {
    /// The engine is asked when the window opens and again when its status line
    /// is clicked. Asking inside a frame would stall the render on a socket read.
    engine: EngineStatus,
    projects: Vec<Project>,
    /// Why the Project list could not be read, when it could not be.
    projects_error: Option<String>,
    selected_project: Option<usize>,
    memory: MemoryScreen,
    reviews: ReviewsScreen,
    shell: Shell,
    /// The pane header's actions take focus here. F6 is the Windows key for
    /// moving between a window's regions, and it is the only way out of an
    /// editor that consumes Tab.
    actions_focus: FocusHandle,
    /// The destinations, which are a region of their own: a rail that only a
    /// pointer can reach is a rail half the readers cannot use.
    rail_focus: FocusHandle,
    /// The form shown while the daemon has no Server session.
    sign_in: SignInScreen,
    signed_in: bool,
    /// Which debounced store owns the editor. A store that a later keystroke
    /// has superseded must not report its result as the editor's state.
    save_generation: u64,
    /// Dropping it stops watching the system's light or dark preference.
    _appearance: Subscription,
}

impl DesktopApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = engine::engine_status();
        let (projects, projects_error) = read_projects();
        let (checkout, checkout_error) = match projects.first() {
            Some(project) => read_checkout(&project.project_id),
            None => (None, None),
        };
        let mut reviews = ReviewsScreen::new(cx);
        reviews.set_project(
            projects.first().map(|project| project.project_id.clone()),
            cx,
        );
        if let Some(checkout) = &checkout {
            reviews.set_published(checkout);
        }
        let memory = MemoryScreen::new(window, cx, checkout, checkout_error);
        // A daemon with no session refuses every Server request, and that
        // refusal is the only signed-out signal there is.
        let signed_in = projects_error
            .as_deref()
            .is_none_or(|error| !engine::missing_session(error));
        let server_url = engine::configured_server_url().unwrap_or_default();
        let sign_in = SignInScreen::new(window, cx, &server_url);
        // The window follows the system's light or dark preference, now and
        // whenever it changes: a client that stays white on a dark desktop is
        // a client nobody wants open.
        Theme::sync_system_appearance(Some(window), cx);
        let appearance = cx.observe_window_appearance(window, |_app, _window, cx| {
            Theme::sync_system_appearance(None, cx);
        });
        let mut app = Self {
            engine,
            selected_project: signed_in.then_some(0),
            projects,
            projects_error,
            memory,
            reviews,
            shell: Shell::new(),
            actions_focus: cx.focus_handle(),
            rail_focus: cx.focus_handle(),
            sign_in,
            signed_in,
            save_generation: 0,
            _appearance: appearance,
        };
        // A Project that already holds a proposal must show it on the first
        // frame: the tree marks it and the pane header offers to review it.
        app.refresh_drafts(cx);
        // The pane's tools are a region of the window (F6 walks it), so the
        // window owns the handle and the screen draws from it.
        app.memory.set_tools_focus(app.actions_focus.clone());
        // Now that the handle is the one the pane tracks, the window gets a
        // focus: without one it drops every key, and the menu builders and
        // dialogs below never hear anything.
        app.memory.focus_open_document(window, cx);
        // The band's own close control flushes what the panes still hold; a
        // window the compositor closes goes through here instead, and the same
        // flush runs before it is allowed to go. It is registered once, as the
        // window opens: the platform refuses a callback registered while it is
        // drawing.
        let flushing = cx.entity();
        window.on_window_should_close(cx, move |_window, cx| {
            flushing.update(cx, |app, cx| app.flush_pending_saves(cx));
            true
        });
        app
    }

    /// The Memory screen, which the tree reaches through its own selection
    /// notification.
    pub fn memory(&mut self) -> &mut MemoryScreen {
        &mut self.memory
    }

    /// The same screen, for a reader rather than a writer: the tree's menu is
    /// built while the window renders, and it only needs to know what a row has.
    pub fn memory_ref(&self) -> &MemoryScreen {
        &self.memory
    }

    /// The one command the list column's right side offers: the Project's
    /// settings. Anything more is more than a reader needs above the work.
    fn settings_button(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("project-settings")
            .ghost()
            .icon(Icon::default().path("icons/settings.svg"))
            .tooltip("Project settings")
            .on_click(cx.listener(|app, _event, window, cx| app.open_project_settings(window, cx)))
            .into_any_element()
    }

    /// Opens the Project settings dialog. Settings do not replace the work: a
    /// reader who loses their document to a settings pane has to work out how to
    /// get it back, and a dialog never takes it away.
    pub fn open_project_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self
            .selected_project
            .and_then(|index| self.projects.get(index))
        else {
            return;
        };
        let health = match &self.engine {
            EngineStatus::Connected(health) => Some(health),
            EngineStatus::Unreachable(_) => None,
        };
        let settings = ProjectSettings {
            project: project.name.clone(),
            project_id: project.project_id.clone(),
            storage: engine::project_storage(&project.project_id),
            server: health.map(|health| health.server_url.clone()),
            daemon: health
                .map(|health| health.daemon_version.clone())
                .unwrap_or_else(|| "not answering".to_owned()),
            log_dir: health.map(|health| health.log_dir.clone()),
        };
        let app = cx.entity().downgrade();
        let view = cx.new(|_| ProjectSettingsDialog::new(app, settings));
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let view = view.clone();
            dialog
                .title("Project settings")
                .w(px(560.))
                .keyboard(true)
                .content(move |content, _window, _cx| content.child(view.clone()))
                .footer(div())
                .footer(div())
        });
    }

    /// Opens a document from the tree's menu. A menu comes with a window, so
    /// the tab can be made now rather than on the next frame.
    pub fn open_document(
        &mut self,
        path: &str,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.memory.open_now(path, Some(mode), window, cx) {
            cx.notify();
        }
    }

    /// Asks for a Review of one document, whether or not it is the open one.
    pub fn request_review_for(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.memory.open_now(path, None, window, cx) {
            self.request_review(window, cx);
        }
    }

    /// Throws away the draft that carries a document's edits. macOS does this
    /// without asking; the published document is untouched either way.
    pub fn discard_draft_for(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some((draft_id, resource_id)) = self.memory.draft_for_path(path) else {
            return;
        };
        let Some(project_id) = self.memory.project_id().map(str::to_owned) else {
            return;
        };
        let discarded = resource_id.clone();
        let work = cx
            .background_executor()
            .spawn(async move { engine::discard_draft(&project_id, &draft_id, &resource_id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |app, cx| {
                match result {
                    Ok(response) => crate::logging::info(&format!(
                        "discarded {} for {}",
                        response.draft_id, discarded
                    )),
                    Err(error) => crate::logging::error(&format!(
                        "could not discard the draft for {discarded}: {error}"
                    )),
                }
                app.refresh_drafts(cx);
                app.reload_memory(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts a new Memory document in a folder, which is the one command the
    /// tree offers on a folder row.
    pub fn open_new_memory_dialog(
        &mut self,
        folder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        NewMemoryDialog::open(cx.entity().downgrade(), folder, "untitled.md", window, cx);
    }

    /// Writes a new document as a draft. The file exists for the Project once
    /// the Review carrying it is merged.
    pub fn create_memory(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(project_id) = self
            .selected_project
            .and_then(|index| self.projects.get(index))
            .map(|project| project.project_id.clone())
        else {
            return;
        };
        let commit = self.memory.commit_id().map(str::to_owned);
        let path = path.to_owned();
        // A file with its name in it and nothing else: the reader writes it.
        let title = path
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .trim_end_matches(".md")
            .to_owned();
        let content = format!("# {title}\n");
        let asked = path.clone();
        let work = cx.background_executor().spawn(async move {
            engine::create_document(&project_id, commit.as_deref(), &asked, &content).map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let created = result.is_ok();
            this.update(cx, |app, cx| {
                app.document_changed("created", &path, result, cx);
                // The file is a draft until a Review carries it, so it arrives
                // as a proposal row on the read above and opens to be written
                // in, which is what macOS does with a new Memory document.
                if created {
                    app.memory.open_when_loaded(&path, Some(Mode::Edit));
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks for a new name for a folder, which renames every document below it.
    pub fn open_rename_folder_dialog(
        &mut self,
        folder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        RenameFolderDialog::open(cx.entity().downgrade(), folder, window, cx);
    }

    /// Renames a folder by renaming each document below it, in path order, so
    /// that the relative paths inside the folder survive.
    pub fn rename_folder(&mut self, folder: &str, name: &str, cx: &mut Context<Self>) {
        let plan = self.memory.folder_rename_plan(folder, name);
        if plan.is_empty() {
            return;
        }
        let what = format!("{folder} renamed to {name}");
        let calls: Vec<_> = plan
            .into_iter()
            .map(|(edit, new_path)| {
                Box::new(move || engine::rename_document(&edit, &new_path).map(|_| ()))
                    as Box<dyn FnOnce() -> Result<(), String> + Send>
            })
            .collect();
        self.run_plan(what, calls, cx);
    }

    /// Starts a Project's Memory: the guidelines and a folder for each kind of
    /// knowledge, which is what macOS offers an empty Memory. Every document is
    /// proposed as a draft in one go, so the whole starting point is one Review,
    /// and the folder a Project already uses is left alone.
    pub fn set_up_guidelines(&mut self, cx: &mut Context<Self>) {
        let Some(project_id) = self.memory.project_id().map(str::to_owned) else {
            return;
        };
        let commit = self.memory.commit_id().map(str::to_owned);
        let starters = guidelines::starters(&self.memory.paths());
        let work = cx.background_executor().spawn(async move {
            for starter in &starters {
                engine::create_document(
                    &project_id,
                    commit.as_deref(),
                    &starter.path,
                    starter.body,
                )?;
            }
            Ok::<usize, String>(starters.len())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |app, cx| {
                match &result {
                    Ok(count) => crate::logging::info(&format!(
                        "started this Project's Memory: {count} documents proposed"
                    )),
                    Err(error) => {
                        crate::logging::error(&format!("could not start Memory: {error}"))
                    }
                }
                if result.is_ok() {
                    // The reader is here to write the guidelines, which is what
                    // macOS opens after its own setup.
                    app.memory
                        .open_when_loaded(guidelines::GUIDELINES_PATH, Some(Mode::Edit));
                }
                app.refresh_drafts(cx);
                app.reload_memory(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks the daemon to try this Project's draft uploads again, which is what a
    /// row offers when its draft could not be uploaded. macOS makes the same
    /// daemon call and calls it "Retry Draft Sync".
    pub fn retry_draft_sync(&mut self, cx: &mut Context<Self>) {
        let Some(project_id) = self.memory.project_id().map(str::to_owned) else {
            return;
        };
        self.storage_action(
            "asked the daemon to retry this Project's drafts",
            move || engine::sync_now(&project_id),
            cx,
        );
    }

    /// Stores what every open pane still holds before the window goes.
    ///
    /// A pane whose store is waiting for the pause has an edit the engine has not
    /// seen, and the pause will not come: the window is closing. macOS asks about
    /// that text; this client's model is a store after a pause, so the same edit
    /// goes now instead of being asked about.
    pub fn flush_pending_saves(&mut self, cx: &mut Context<Self>) {
        for resource_id in self.memory.pending_saves() {
            let Some(edit) = self.document_edit(&resource_id, cx) else {
                continue;
            };
            // The store the pane is still waiting for carries an older text and
            // is cancelled by the generation it was asked with.
            if let Some(pane) = self.memory.pane_for_resource_mut(&resource_id) {
                self.save_generation += 1;
                pane.set_generation(self.save_generation);
                pane.set_save_state(SaveState::Saving);
            }
            let result = engine::store_document(&edit);
            match &result {
                Ok(_) => {
                    crate::logging::info(&format!("stored {} as the window closed", edit.path))
                }
                Err(error) => crate::logging::error(&format!(
                    "could not store {} as the window closed: {error}",
                    edit.path
                )),
            }
            if let Some(pane) = self.memory.pane_for_resource_mut(&resource_id) {
                pane.set_save_state(match &result {
                    Ok(_) => SaveState::Saved,
                    Err(error) => SaveState::Failed(error.clone()),
                });
            }
        }
    }

    /// Opens several documents at once, which is what the menu on a set of rows
    /// offers. Each becomes its own tab, so a batch is the same tabs a reader
    /// would have opened one by one.
    pub fn open_documents(
        &mut self,
        paths: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for path in paths {
            self.memory.open_now(path, None, window, cx);
        }
        cx.notify();
    }

    /// Asks for one Review of every draft a set of rows carries, which is what
    /// macOS calls "Request Review for N Changes": the Review names each draft,
    /// and the Server decides them together.
    pub fn request_review_for_selection(
        &mut self,
        paths: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edits = self.memory.review_plan(paths);
        if edits.is_empty() {
            return;
        }
        // The sheet starts from a title about the batch, which is what macOS's
        // "Update memory" / "Update N memories" does.
        let title = match edits.len() {
            1 => "Update memory".to_owned(),
            count => format!("Update {count} memories"),
        };
        let (title, description) = document::review_fields(title, window, cx);
        document::open_review_sheet(
            edits,
            false,
            title,
            description,
            cx.entity().downgrade(),
            window,
            cx,
        );
    }

    /// Confirms that a set of rows should be proposed for deletion — one
    /// document, a folder, or a batch the reader selected. A deletion is a draft
    /// like any other, so this asks once and then lets the Review decide.
    pub fn open_delete_dialog(
        &mut self,
        paths: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Deleting is about what the Project holds: a document that only exists
        // as a proposal is thrown away by a discard, which is the dialog its own
        // row offers.
        let documents: Vec<String> = self
            .memory
            .delete_plan(paths)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        if documents.is_empty() {
            return;
        }
        let (title, message, confirm) = match (self.folder_row(paths), documents.len()) {
            (Some(folder), count) => (
                "Delete Folder?",
                format!(
                    "This proposes that the {count} memories in {folder} be deleted. Each proposal is saved as a draft and takes effect for the Project after review and merge."
                ),
                "Delete folder",
            ),
            (None, 1) => (
                "Delete File?",
                format!(
                    "This proposes that {} be deleted. The proposal is saved as a draft and takes effect for the Project after review and merge.",
                    documents[0]
                ),
                "Delete",
            ),
            (None, count) => (
                "Delete Files?",
                format!(
                    "This proposes that these {count} memories be deleted. Each proposal is saved as a draft and takes effect for the Project after review and merge.\n\n{}",
                    listing(&documents)
                ),
                "Delete files",
            ),
        };
        ConfirmDialog::open(
            cx.entity().downgrade(),
            title,
            message,
            confirm,
            DialogAction::DeleteDocuments {
                paths: paths.to_vec(),
            },
            window,
            cx,
        );
    }

    /// Confirms throwing away every draft a set of rows carries.
    pub fn open_discard_dialog(
        &mut self,
        paths: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.memory.discard_plan(paths).len();
        if count == 0 {
            return;
        }
        let (title, message) = match self.folder_row(paths) {
            Some(folder) => (
                "Discard drafts in this folder?",
                format!(
                    "This throws away the {count} drafts below {folder}. The published Memory is untouched."
                ),
            ),
            None => (
                "Discard drafts?",
                format!(
                    "This throws away the {count} drafts the selection carries. The published Memory is untouched."
                ),
            ),
        };
        ConfirmDialog::open(
            cx.entity().downgrade(),
            title,
            message,
            "Discard drafts",
            DialogAction::DiscardDrafts {
                paths: paths.to_vec(),
            },
            window,
            cx,
        );
    }

    /// What a log line calls a set of rows: a folder by name, a batch by how
    /// many documents it turned out to hold.
    fn batch_name(&self, paths: &[String], documents: usize) -> String {
        match self.folder_row(paths) {
            Some(folder) => folder,
            None => format!("{documents} documents"),
        }
    }

    /// The folder a set of rows is, when it is exactly one folder. A command
    /// about a folder names it, which is what this client's dialogs have said
    /// since before a batch could be selected at all.
    fn folder_row(&self, paths: &[String]) -> Option<String> {
        let [path] = paths else { return None };
        self.memory
            .menu_target(path)
            .is_some_and(|target| target.is_folder)
            .then(|| path.clone())
    }

    /// Runs one daemon call per item of a plan, in order, and reports how many
    /// of them it managed before the first refusal. A folder's documents, a
    /// batch the reader selected, and a Review's several drafts are all plans.
    fn run_plan(
        &mut self,
        what: String,
        calls: Vec<Box<dyn FnOnce() -> Result<(), String> + Send>>,
        cx: &mut Context<Self>,
    ) {
        let total = calls.len();
        let work = cx.background_executor().spawn(async move {
            let mut done = 0usize;
            for call in calls {
                if let Err(error) = call() {
                    return Err((done, error));
                }
                done += 1;
            }
            Ok(done)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |app, cx| {
                match result {
                    Ok(done) => crate::logging::info(&format!("{what}: {done} of {total}")),
                    Err((done, error)) => {
                        crate::logging::error(&format!("{what}: {done} of {total}, then {error}"))
                    }
                }
                app.refresh_drafts(cx);
                app.reload_memory(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks for a new name for a document. A dialog rather than a field inside
    /// the row: a field that appears in a tree is a field the reader can lose.
    pub fn open_rename_dialog(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.memory.edit_for_path(path) else {
            return;
        };
        RenameDialog::open(cx.entity().downgrade(), path, edit, window, cx);
    }

    /// Confirms that a document should be proposed for deletion. The deletion is
    /// a draft like any other, so this asks once and then lets the Review
    /// decide.
    /// Writes a new path for a document as a draft operation.
    pub fn rename_document(&mut self, edit: DocumentEdit, new_path: &str, cx: &mut Context<Self>) {
        let path = new_path.to_owned();
        let asked = new_path.to_owned();
        let work = cx
            .background_executor()
            .spawn(async move { engine::rename_document(&edit, &asked) });
        let reported = path.clone();
        cx.spawn(async move |this, cx| {
            let result = work.await.map(|_| ());
            this.update(cx, |app, cx| {
                app.document_changed("renamed to", &reported, result, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Runs what a confirmed dialog asked for.
    pub fn run_dialog_action(&mut self, action: DialogAction, cx: &mut Context<Self>) {
        match action {
            DialogAction::ResetStorage {
                project_id,
                revision,
            } => {
                self.storage_action(
                    "reset the Memory location",
                    move || engine::reset_project_storage(&project_id, revision),
                    cx,
                );
            }
            DialogAction::ClearCache {
                project_id,
                revision,
            } => {
                self.storage_action(
                    "cleared the Project cache",
                    move || engine::clear_project_cache(&project_id, revision),
                    cx,
                );
            }
            DialogAction::DeleteDocuments { paths } => {
                let plan = self.memory.delete_plan(&paths);
                if plan.is_empty() {
                    return;
                }
                let what = self.batch_name(&paths, plan.len());
                let calls: Vec<_> = plan
                    .into_iter()
                    .map(|(_, edit)| {
                        Box::new(move || engine::delete_document(&edit).map(|_| ()))
                            as Box<dyn FnOnce() -> Result<(), String> + Send>
                    })
                    .collect();
                self.run_plan(format!("{what} proposed for deletion"), calls, cx);
            }
            DialogAction::DiscardDrafts { paths } => {
                let plan = self.memory.discard_plan(&paths);
                if plan.is_empty() {
                    return;
                }
                let what = self.batch_name(&paths, plan.len());
                let project_id = self.memory.project_id().unwrap_or_default().to_owned();
                let calls: Vec<_> = plan
                    .into_iter()
                    .map(|(_, draft_id, resource_id)| {
                        let project_id = project_id.clone();
                        Box::new(move || {
                            engine::discard_draft(&project_id, &draft_id, &resource_id).map(|_| ())
                        }) as Box<dyn FnOnce() -> Result<(), String> + Send>
                    })
                    .collect();
                self.run_plan(format!("drafts discarded in {what}"), calls, cx);
            }
            DialogAction::SyncNow { project_id } => {
                self.storage_action(
                    "asked the daemon to sync",
                    move || engine::sync_now(&project_id),
                    cx,
                );
            }
        }
    }

    /// One command about where the Project's Memory lives, and what it says when
    /// it is done.
    fn storage_action(
        &mut self,
        what: &'static str,
        action: impl FnOnce() -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let work = cx.background_executor().spawn(async move { action() });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |app, cx| {
                match result {
                    Ok(()) => crate::logging::info(what),
                    Err(error) => crate::logging::error(&format!("{what}: {error}")),
                }
                app.reload_memory(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What a rename or a deletion produced: the Project is read again, because
    /// both change what its Memory holds.
    fn document_changed(
        &mut self,
        what: &str,
        path: &str,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => crate::logging::info(&format!("{path} {what}")),
            Err(error) => crate::logging::error(&format!("could not change {path}: {error}")),
        }
        self.refresh_drafts(cx);
        self.reload_memory(cx);
        cx.notify();
    }

    /// Opens the Review sheet for the document in front, which is what the
    /// pane's overflow offers and what the tree's menu offers per row.
    pub fn request_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.memory.render_target() else {
            return;
        };
        let Some(pane) = self.memory.active_pane() else {
            return;
        };
        let (edit, store) = pane.review_edit(&target, cx);
        document::open_review_sheet(
            vec![edit],
            store,
            pane.review_title(),
            pane.review_description(),
            cx.entity().downgrade(),
            window,
            cx,
        );
    }

    /// The sections are the shell's, so the window only has to be told which one
    /// is open.
    pub fn select_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.shell.set_section(section);
        // The Reviews queue is read when the section is first opened, not at
        // every switch: the Server is asked, and its answer does not change by
        // being looked at again.
        if section == Section::Reviews && self.reviews.needs_reading() {
            self.refresh_reviews(cx);
        }
        cx.notify();
    }

    pub fn toggle_projects(&mut self, cx: &mut Context<Self>) {
        self.shell.toggle_projects();
        cx.notify();
    }

    pub fn close_projects(&mut self, cx: &mut Context<Self>) {
        self.shell.close_projects();
        cx.notify();
    }

    /// A Project was picked from the list header's panel.
    pub fn choose_project(&mut self, index: usize, cx: &mut Context<Self>) {
        self.shell.close_projects();
        self.select_project(index, cx);
    }

    /// Selecting a Project reads its Memory. That read is a socket call to the
    /// daemon, which is why it happens on the click rather than every frame.
    fn select_project(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_project = Some(index);
        if let Some(project) = self.projects.get(index) {
            let (checkout, error) = read_checkout(&project.project_id);
            // Another Project is another queue, so the Reviews screen is told
            // before its list is read — and before the new checkout replaces
            // the old Project's documents.
            self.reviews
                .set_project(Some(project.project_id.clone()), cx);
            if let Some(checkout) = &checkout {
                self.reviews.set_published(checkout);
            }
            self.memory.set_checkout(checkout, error, cx);
            self.refresh_drafts(cx);
            if self.shell.section() == Section::Reviews {
                self.refresh_reviews(cx);
            }
        }
        cx.notify();
    }

    /// Moves the keyboard to the next region, which is what F6 is for on this
    /// platform. The order is the one a reader walks the window in: the list,
    /// the work, and the actions over it.
    pub fn cycle_focus(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        const REGIONS: [Region; 4] = [Region::Rail, Region::List, Region::Detail, Region::Actions];
        let current = if self.rail_focus.is_focused(window) {
            Region::Rail
        } else if self.list_focused(window) {
            Region::List
        } else if self.actions_focus.is_focused(window) {
            Region::Actions
        } else {
            Region::Detail
        };
        let index = REGIONS
            .iter()
            .position(|region| *region == current)
            .unwrap_or(Region::Detail as usize);
        let next = REGIONS[(index as isize + step).rem_euclid(REGIONS.len() as isize) as usize];
        match next {
            Region::Rail => window.focus(&self.rail_focus, cx),
            Region::List if self.shell.section() == Section::Reviews => {
                self.reviews.focus_list(window, cx)
            }
            Region::List => self.memory.focus_list(window, cx),
            Region::Detail => self.focus_content(window, cx),
            Region::Actions => self.focus_actions(window, cx),
        }
        cx.notify();
    }

    /// Moves the file tree's selection, which is what the arrow keys do in a
    /// list. The keys are handled here rather than on the tree because the tree
    /// component gives a click its own focus handle; this one is what F6 gives
    /// the keyboard to, and both arrive at the same selection.
    pub fn move_in_list(&mut self, movement: Move, cx: &mut Context<Self>) {
        if self.shell.section() == Section::Memory {
            self.memory.move_selection(movement, cx);
            return;
        }
        if self.shell.section() == Section::Reviews {
            // Selecting a Review is what opens it, the same way selecting a
            // document in Memory opens that.
            let step = match movement {
                Move::Step(step) => step,
                Move::First => isize::MIN,
                Move::Last => isize::MAX,
            };
            if let Some(review_id) = self.reviews.selection_after(step) {
                self.open_review(&review_id, cx);
            }
        }
    }

    /// Moves between the destinations, which is what the arrows do once the
    /// rail has the keyboard. Moving is choosing: a rail row is the section it
    /// opens, the same way a list row is the document it opens.
    pub fn move_in_rail(&mut self, step: isize, cx: &mut Context<Self>) {
        let sections = Section::ALL;
        let current = sections
            .iter()
            .position(|section| *section == self.shell.section())
            .unwrap_or(0);
        let index = (current as isize + step).clamp(0, sections.len() as isize - 1) as usize;
        let section = sections[index];
        if section != self.shell.section() {
            self.select_section(section, cx);
        }
    }

    /// Whether the open section's list has the keyboard, which is what tells
    /// F6 where it is and the arrow keys where they are.
    fn list_focused(&self, window: &Window) -> bool {
        match self.shell.section() {
            Section::Memory => self.memory.list_focused(window),
            Section::Reviews => self.reviews.list_focused(window),
            _ => false,
        }
    }

    /// Puts focus on the window's actions, which is where F6 ends up.
    pub fn focus_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.actions_focus, cx);
        cx.notify();
    }

    /// Gives the caret back to the open document, which is what Shift+F6 does.
    pub fn focus_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.section() == Section::Memory
            && let Some(pane) = self.memory.active_pane()
        {
            pane.focus_editor(window, cx);
            cx.notify();
        }
    }

    /// A tab was picked from the strip of open documents.
    pub fn select_tab(&mut self, resource_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.section() == Section::Memory {
            self.memory.select_tab(resource_id, window, cx);
        }
    }

    /// A tab's close button, or Ctrl+W on the tab in front. macOS closes the
    /// active tab with Command-W, so the same act takes this platform's
    /// Command key.
    pub fn close_tab(&mut self, resource_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.section() == Section::Memory {
            self.memory.close_tab(resource_id, window, cx);
        }
    }

    pub fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.section() == Section::Memory {
            self.memory.close_active_tab(window, cx);
        }
    }

    /// Ctrl+Tab walks the open documents, which is this platform's key for it.
    pub fn cycle_tab(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.section() == Section::Memory {
            self.memory.cycle_tab(step, window, cx);
        }
    }

    /// The band's back arrow, and Alt+Left with it: the document the reader
    /// came from. macOS walks the same stacks with the arrow beside its
    /// document's name.
    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.section() == Section::Memory {
            self.memory.go_back(window, cx);
        }
    }

    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.section() == Section::Memory {
            self.memory.go_forward(window, cx);
        }
    }

    /// The window's primary action: ask for a Review of the open document. The
    /// pane header's button and Enter, once the actions have focus, both land
    /// here, so the mouse and the keyboard cannot drift apart.
    pub fn run_primary_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_run_primary_action() {
            return;
        }
        if self.shell.section() == Section::Reviews {
            self.merge_review(cx);
            return;
        }
        if self.shell.section() == Section::Memory {
            self.toggle_document_edit(cx);
            return;
        }

        let Some(target) = self.memory.render_target() else {
            return;
        };
        let Some(pane) = self.memory.active_pane() else {
            return;
        };
        let (edit, store) = pane.review_edit(&target, cx);
        document::open_review_sheet(
            vec![edit],
            store,
            pane.review_title(),
            pane.review_description(),
            cx.entity().downgrade(),
            window,
            cx,
        );
    }

    fn can_run_primary_action(&self) -> bool {
        match self.shell.section() {
            // The pane's primary tool is editing, which is always available for
            // a document that is open.
            Section::Memory => self.memory.active_pane().is_some(),
            Section::Reviews => self.reviews.can_approve() || self.reviews.can_merge(),
            _ => false,
        }
    }

    /// A keystroke landed in the editor. The store waits for a pause in typing,
    /// which is what the macOS client's 600ms debounce is for: every store is a
    /// socket call into the daemon and an upload behind it.
    ///
    /// The edit is captured now rather than when the pause ends, because by then
    /// the reader may have opened another document or another Project, and this
    /// text belongs to the one it was typed in.
    pub fn document_edited(&mut self, editor: EntityId, cx: &mut Context<Self>) {
        let Some(resource_id) = self.memory.resource_for_editor(editor) else {
            return;
        };
        self.save_generation += 1;
        let generation = self.save_generation;
        if let Some(pane) = self.memory.pane_for_resource_mut(&resource_id) {
            pane.set_save_state(SaveState::Pending);
            pane.set_generation(generation);
        }
        let Some(edit) = self.document_edit(&resource_id, cx) else {
            return;
        };
        let pause = cx.background_executor().timer(SAVE_DELAY);
        cx.spawn(async move |this, cx| {
            pause.await;
            this.update(cx, |app, cx| app.save_document(generation, edit, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Stores one captured edit through the daemon. The daemon queues the
    /// operation and uploads it, so this returns before the Server has it; the
    /// Review request is what waits for the upload.
    fn save_document(&mut self, generation: u64, edit: DocumentEdit, cx: &mut Context<Self>) {
        // A later keystroke in the same document has already asked for a newer
        // store, and that one carries the newer text. A document whose tab has
        // closed has no pane to ask, and its last edit still belongs to the
        // engine, so it goes.
        if let Some(pane) = self.memory.pane_for_resource(&edit.resource_id)
            && pane.generation() != generation
        {
            return;
        }
        if let Some(pane) = self.memory.pane_for_resource_mut(&edit.resource_id) {
            pane.set_save_state(SaveState::Saving);
            cx.notify();
        }
        let content = edit.content.clone();
        let resource_id = edit.resource_id.clone();
        let work = cx
            .background_executor()
            .spawn(async move { engine::store_document(&edit) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |app, cx| {
                app.document_stored(generation, &resource_id, &content, result, cx)
            })
            .ok();
        })
        .detach();
    }

    /// What one store produced. The text is recorded as stored even when a later
    /// keystroke has already asked for another store, because that is what
    /// "unsaved" is measured against; only the newest store of the open document
    /// decides what the window reports.
    fn document_stored(
        &mut self,
        generation: u64,
        resource_id: &str,
        content: &str,
        result: Result<DaemonDraftOperationResponse, String>,
        cx: &mut Context<Self>,
    ) {
        // A store that landed may have created or advanced a draft, and the tree
        // marks every document whose draft moved, so the list is re-read
        // whichever document the reader is now looking at.
        let stored = match &result {
            Ok(response) => Some(response.draft_id.clone()),
            Err(_) => None,
        };
        match (&result, &stored) {
            (Ok(_), Some(draft_id)) => {
                crate::logging::info(&format!("stored an edit of {resource_id} into {draft_id}"))
            }
            (Err(error), _) => crate::logging::error(&format!(
                "could not store an edit of {resource_id}: {error}"
            )),
            _ => {}
        }
        if let Some(draft_id) = stored {
            self.refresh_drafts(cx);
            self.follow_upload(draft_id, cx);
        }
        // The rest is the document's own pane, and a store may finish after the
        // reader has left it or closed it. The engine has the text either way;
        // a pane reports only what it is showing.
        let Some(pane) = self.memory.pane_for_resource_mut(resource_id) else {
            return;
        };
        pane.accept_text(content.to_owned());
        if pane.generation() == generation {
            match result {
                Ok(_) => pane.set_save_state(SaveState::Saved),
                Err(error) => pane.set_save_state(SaveState::Failed(error)),
            }
        }
        cx.notify();
    }

    /// Waits for the daemon to upload a just-stored draft and re-reads the list,
    /// so the window stops saying "uploading" the moment that stops being true.
    /// macOS refreshes the draft after a store for the same reason; the daemon
    /// pushes no event here, so the client asks once.
    fn follow_upload(&mut self, draft_id: String, cx: &mut Context<Self>) {
        let work = cx
            .background_executor()
            .spawn(async move { engine::wait_for_upload(&draft_id) });
        cx.spawn(async move |this, cx| {
            // A draft that cannot be uploaded reports itself in the list the
            // store already asked for, so only success needs reporting here.
            if work.await.is_ok() {
                this.update(cx, |app, cx| app.refresh_drafts(cx)).ok();
            }
        })
        .detach();
    }

    /// Turns editing on for the document in front, or off again: a tool the
    /// reader picks up, not a mode the window is in.
    pub fn toggle_document_edit(&mut self, cx: &mut Context<Self>) {
        let mode = match self.memory.active_pane().map(|pane| pane.mode()) {
            Some(Mode::Edit) => Mode::Preview,
            _ => Mode::Edit,
        };
        self.set_document_mode(mode, cx);
    }

    /// The same for the diff, which is what an edit looks like against what the
    /// Project publishes.
    pub fn show_document_diff(&mut self, cx: &mut Context<Self>) {
        let mode = match self.memory.active_pane().map(|pane| pane.mode()) {
            Some(Mode::Diff) => Mode::Preview,
            _ => Mode::Diff,
        };
        self.set_document_mode(mode, cx);
    }

    pub fn set_document_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if let Some(pane) = self.memory.active_pane_mut() {
            pane.set_mode(mode);
        }
        cx.notify();
    }

    /// A Review was created for the draft the sheet held. The macOS client
    /// switches to its Reviews section here; this client reports the Review and
    /// leaves the reader in the document, because the Reviews section is not
    /// built yet.
    pub fn review_requested(&mut self, review: Review, cx: &mut Context<Self>) {
        crate::logging::info(&format!(
            "review {} requested for {}",
            review.review_id, review.title
        ));
        self.memory.set_notice(Some(Notice {
            text: format!(
                "Review {} requested · {}",
                ui::shorten(&review.review_id, 8),
                review.title
            ),
        }));
        self.refresh_drafts(cx);
        cx.notify();
    }

    /// The edit the pane would store: its text, the document it belongs to, and
    /// the draft that already carries it when there is one.
    fn document_edit(&self, resource_id: &str, cx: &App) -> Option<DocumentEdit> {
        let project = self.projects.get(self.selected_project?)?;
        let document = self.memory.document_for_resource(resource_id)?;
        let draft = self.memory.draft_for_resource(resource_id);
        let pane = self.memory.pane_for_resource(resource_id)?;
        Some(DocumentEdit {
            project_id: project.project_id.clone(),
            base_commit_id: draft
                .and_then(|draft| draft.base_commit_id.clone())
                .or_else(|| self.memory.commit_id().map(str::to_owned)),
            draft_id: draft.map(|draft| draft.draft_id.clone()),
            resource_id: document.resource_id.clone(),
            published: document.published,
            path: document.path.clone(),
            content: pane.text(cx),
        })
    }

    /// Re-reads the drafts of the selected Project. The daemon is asked in the
    /// background because the answer is a socket call, and a draft list that
    /// could not be read is not a failed edit: the status bar already reports
    /// whether the engine answers at all.
    fn refresh_drafts(&mut self, cx: &mut Context<Self>) {
        let Some(project_id) = self
            .selected_project
            .and_then(|index| self.projects.get(index))
            .map(|project| project.project_id.clone())
        else {
            return;
        };
        let work = cx
            .background_executor()
            .spawn(async move { engine::drafts(&project_id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |app, cx| app.drafts_refreshed(result, cx))
                .ok();
        })
        .detach();
    }

    fn drafts_refreshed(
        &mut self,
        result: Result<Vec<DaemonDraftSummary>, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(drafts) => self.memory.set_drafts(drafts, cx),
            Err(error) => crate::logging::error(&format!("could not read the drafts: {error}")),
        }
    }

    /// Re-reads everything a session unlocks.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.engine = engine::engine_status();
        let (projects, projects_error) = read_projects();
        let (checkout, checkout_error) = match projects.first() {
            Some(project) => read_checkout(&project.project_id),
            None => (None, None),
        };
        self.selected_project = (!projects.is_empty()).then_some(0);
        if let Some(checkout) = &checkout {
            self.reviews.set_published(checkout);
        }
        self.reviews.set_project(
            projects.first().map(|project| project.project_id.clone()),
            cx,
        );
        self.projects = projects;
        self.projects_error = projects_error;
        self.memory.set_checkout(checkout, checkout_error, cx);
        self.refresh_drafts(cx);
        if self.shell.section() == Section::Reviews {
            self.refresh_reviews(cx);
        }
    }

    /// Reads the Project's Reviews. The daemon holds the session, so this is a
    /// socket call, and it happens on the click that opens the section rather
    /// than every frame.
    fn refresh_reviews(&mut self, cx: &mut Context<Self>) {
        let Some(project_id) = self.reviews.project_id().map(str::to_owned) else {
            return;
        };
        self.reviews.begin_list_read();
        cx.notify();
        let work = cx
            .background_executor()
            .spawn(async move { engine::reviews(&project_id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |app, cx| app.reviews_loaded(result, cx))
                .ok();
        })
        .detach();
    }

    fn reviews_loaded(
        &mut self,
        result: Result<Vec<engine::Review>, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(reviews) => {
                crate::logging::info(&format!("read {} Reviews", reviews.len()));
                self.reviews.set_reviews(reviews, None, cx);
            }
            Err(error) => {
                crate::logging::error(&format!("could not read the Reviews: {error}"));
                self.reviews.set_list_error(error, cx);
            }
        }
    }

    /// Opens one Review: the queue's selection and the read that fills its
    /// detail both start here, whether a click or the keyboard asked.
    pub fn open_review(&mut self, review_id: &str, cx: &mut Context<Self>) {
        if self.reviews.open_id() == Some(review_id) {
            return;
        }
        self.reviews.begin_detail_read(review_id.to_owned());
        self.refresh_review(cx);
        cx.notify();
    }

    fn refresh_review(&mut self, cx: &mut Context<Self>) {
        let Some(review_id) = self.reviews.open_id().map(str::to_owned) else {
            return;
        };
        let work = cx
            .background_executor()
            .spawn(async move { engine::review(&review_id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |app, cx| app.review_loaded(result, cx))
                .ok();
        })
        .detach();
    }

    fn review_loaded(
        &mut self,
        result: Result<engine::ReviewDetail, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(detail) => {
                crate::logging::info(&format!(
                    "read Review {} with {} documents",
                    detail.review.review_id,
                    detail.documents.len()
                ));
                self.reviews.set_detail(detail, None, cx);
            }
            Err(error) => {
                crate::logging::error(&format!("could not read a Review: {error}"));
                self.reviews.set_detail_error(error, cx);
            }
        }
    }

    /// Reads one of the open Review's documents, which is what its navigator is
    /// for.
    pub fn show_review_document(&mut self, index: usize, cx: &mut Context<Self>) {
        self.reviews.show_document(index, cx);
    }

    /// Rejects the Review in front. macOS sends no note with a rejection, and
    /// neither does this.
    pub fn reject_review(&mut self, cx: &mut Context<Self>) {
        let Some(review) = self.reviews.open_review().cloned() else {
            return;
        };
        self.run_review_action(
            "rejected",
            move || engine::decide_review(&review, ReviewStatus::Rejected, ""),
            cx,
        );
    }

    /// Approves and publishes the Review in front, which is one transaction on
    /// the Server: an approval that cannot be published is not an approval.
    pub fn merge_review(&mut self, cx: &mut Context<Self>) {
        let Some(review) = self.reviews.open_review().cloned() else {
            return;
        };
        self.run_review_action("merged", move || engine::merge_review(&review), cx);
    }

    fn run_review_action(
        &mut self,
        what: &'static str,
        action: impl FnOnce() -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let work = cx.background_executor().spawn(async move { action() });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |app, cx| app.review_action_finished(what, result, cx))
                .ok();
        })
        .detach();
    }

    /// What a decision answered. A publication changes the Project's Memory as
    /// well as the Review, so both are read again; a refusal is the Server's own
    /// sentence, which is what the reader sees.
    fn review_action_finished(
        &mut self,
        what: &str,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let notice = match result {
            Ok(()) => {
                crate::logging::info(&format!("Review {what}"));
                ReviewNotice {
                    text: format!("This Review is {what}."),
                    failed: false,
                }
            }
            Err(error) => {
                crate::logging::error(&format!("could not {what} a Review: {error}"));
                ReviewNotice {
                    text: error,
                    failed: true,
                }
            }
        };
        self.reviews.set_notice(Some(notice), cx);
        self.refresh_review(cx);
        self.refresh_reviews(cx);
        self.reload_memory(cx);
    }

    /// Re-reads the Project's Memory, which a publication changes.
    fn reload_memory(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self
            .selected_project
            .and_then(|index| self.projects.get(index))
        else {
            return;
        };
        let (checkout, checkout_error) = read_checkout(&project.project_id);
        if let Some(checkout) = &checkout {
            self.reviews.set_published(checkout);
        }
        self.memory.set_checkout(checkout, checkout_error, cx);
    }

    /// The open screen's actions, for the end of its detail header. A screen
    /// hands them to the shell rather than drawing its own, so that a reader
    /// learns one place to look for what the window can do.
    ///
    /// The one action so far belongs to Memory, and the section that would draw
    /// it is the section that has it: a screen with no actions returns nothing
    /// here rather than a button that says so.
    fn actions(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.shell.section() == Section::Reviews {
            return self.review_actions(window, cx);
        }
        if self.shell.section() != Section::Memory {
            return None;
        }
        let focused = self.actions_focus.is_focused(window);
        let ring = if focused {
            cx.theme().ring
        } else {
            transparent_black()
        };
        let enabled = self.can_run_primary_action();
        div()
            .id("window-actions")
            .h_flex()
            .gap_1()
            .items_center()
            .rounded(px(ui::RADIUS))
            .border_1()
            .border_color(ring)
            .p(px(ui::SPACE_XS))
            .track_focus(&self.actions_focus)
            .tab_stop(true)
            .child(
                Button::new("primary-action")
                    .primary()
                    .label("Request review…")
                    .disabled(!enabled)
                    .on_click(
                        cx.listener(|app, _event, window, cx| app.run_primary_action(window, cx)),
                    ),
            )
            .into_any_element()
            .into()
    }

    /// What the open Review can be decided as. macOS keeps the same two
    /// commands in its window toolbar; a command lives in this client's pane
    /// header, and the Server is still the one that decides whether this account
    /// may, so a refusal is reported rather than pre-empted.
    fn review_actions(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.reviews.open_review()?;
        let focused = self.actions_focus.is_focused(window);
        let ring = if focused {
            cx.theme().ring
        } else {
            transparent_black()
        };
        let (can_reject, can_approve, can_merge) = (
            self.reviews.can_reject(),
            self.reviews.can_approve(),
            self.reviews.can_merge(),
        );
        let mut row = div()
            .id("review-actions")
            .h_flex()
            .gap_1()
            .items_center()
            .rounded(px(ui::RADIUS))
            .border_1()
            .border_color(ring)
            .p(px(ui::SPACE_XS))
            .track_focus(&self.actions_focus)
            .tab_stop(true);
        if can_reject {
            row = row.child(
                Button::new("review-reject")
                    .label("Reject…")
                    .on_click(cx.listener(|app, _event, _window, cx| app.reject_review(cx))),
            );
        }
        if can_approve {
            row = row.child(
                Button::new("review-approve")
                    .primary()
                    .label("Approve and merge…")
                    .on_click(cx.listener(|app, _event, _window, cx| app.merge_review(cx))),
            );
        }
        if can_merge {
            row = row.child(
                Button::new("review-merge")
                    .primary()
                    .label("Merge…")
                    .on_click(cx.listener(|app, _event, _window, cx| app.merge_review(cx))),
            );
        }
        Some(row.into_any_element())
    }

    /// What the rail's foot says about the engine this client is talking to.
    fn engine_facts(&self) -> EngineFacts {
        match &self.engine {
            EngineStatus::Connected(health) => EngineFacts {
                connected: true,
                version: health.daemon_version.clone(),
            },
            EngineStatus::Unreachable(_) => EngineFacts {
                connected: false,
                version: String::new(),
            },
        }
    }

    /// Asks the engine again, which is what the rail's chip does when the
    /// client is not talking to anything.
    pub fn recheck_engine(&mut self, cx: &mut Context<Self>) {
        self.engine = engine::engine_status();
        match &self.engine {
            EngineStatus::Connected(health) => crate::logging::info(&format!(
                "engine connected: daemon {} at {}",
                health.daemon_version, health.server_url
            )),
            EngineStatus::Unreachable(reason) => {
                crate::logging::error(&format!("engine unreachable: {reason}"))
            }
        }
        cx.notify();
    }

    /// The open section's list column. A section that has no screen yet says so
    /// rather than drawing an empty column with no explanation.
    fn section_list(
        &self,
        picker: AnyElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.shell.section() {
            Section::Memory => {
                let settings = self.settings_button(cx);
                self.memory.list(picker, Some(settings), window, cx)
            }
            Section::Reviews => self.reviews.list(picker, window, cx),
            other => placeholder(other.list_note(), cx),
        }
    }

    /// Its detail: the work itself. The actions the open screen offers are drawn
    /// in the detail pane's own header, beside what they act on.
    fn section_detail(&self, actions: Option<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        match self.shell.section() {
            Section::Memory => self.memory.detail(&self.actions_focus, cx),
            Section::Reviews => self.reviews.detail(actions, cx),
            other => placeholder(other.detail_note(), cx),
        }
    }

    /// What the window's chrome needs: which Project is open, the list the
    /// picker offers, and the width that decides whether the columns stack.
    fn chrome(&self, window: &Window) -> Chrome<'_> {
        let project = self
            .selected_project
            .and_then(|index| self.projects.get(index))
            .map(|project| project.name.as_str());
        Chrome {
            project,
            projects: &self.projects,
            engine: self.engine_facts(),
            // The account this window is signed in to, as the rail's foot names
            // it: the Server the daemon holds a session with.
            account: match &self.engine {
                EngineStatus::Connected(health) => Some(health.server_url.as_str()),
                EngineStatus::Unreachable(_) => None,
            },
            // Only Memory keeps a history so far; the arrows stay drawn but
            // disabled in a section that has nowhere to go.
            can_go_back: self.shell.section() == Section::Memory && self.memory.can_go_back(),
            can_go_forward: self.shell.section() == Section::Memory && self.memory.can_go_forward(),
            rail_focus: &self.rail_focus,
            rail_focused: self.rail_focus.is_focused(window),
            width: px(0.),
        }
    }
}

/// A slot no screen fills yet: what will live there, taken from the macOS screen
/// it is translated from.
fn placeholder(note: &str, cx: &App) -> AnyElement {
    div()
        .v_flex()
        .h_full()
        .p_4()
        .gap_2()
        .child(ui::message("Not built yet.", cx.theme().muted_foreground))
        .child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child(note.to_owned()),
        )
        .into_any_element()
}

/// How a dialog names a batch: the documents it is about, up to the point where
/// the list stops being readable.
fn listing(paths: &[String]) -> String {
    const SHOWN: usize = 8;
    let mut lines: Vec<String> = paths.iter().take(SHOWN).cloned().collect();
    if paths.len() > SHOWN {
        lines.push(format!("…and {} more", paths.len() - SHOWN));
    }
    lines.join("\n")
}

/// Reads the Project list, keeping the reason when it cannot.
fn read_projects() -> (Vec<Project>, Option<String>) {
    match engine::projects() {
        Ok(projects) => (projects, None),
        Err(error) => (Vec::new(), Some(error)),
    }
}

/// Reads one Project's checkout, keeping the reason when it cannot.
fn read_checkout(project_id: &str) -> (Option<Checkout>, Option<String>) {
    match engine::checkout(project_id) {
        Ok(checkout) => (Some(checkout), None),
        Err(error) => (None, Some(error)),
    }
}

impl Render for DesktopApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Without a session there is nothing to navigate, so the form owns the
        // window rather than sitting inside an empty shell.
        if !self.signed_in {
            // A staged configuration arrives without a window, so the form takes
            // it here, before it draws with its fields.
            self.sign_in.apply_staged(window, cx);
            return self.sign_in.render(cx);
        }
        // A tree click arrives as a notification, which carries no window, so
        // the editor takes its new text at the top of a frame, before the pane
        // draws with it.
        self.memory.apply_pending_open(window, cx);

        let width = window.viewport_size().width;
        let actions = self.actions(window, cx);
        let mut chrome = self.chrome(window);
        chrome.width = width;
        let picker = self.shell.project_picker(&chrome, cx);
        let slots = Slots {
            list: self.section_list(picker, window, cx),
            detail: self.section_detail(actions, cx),
        };
        let shell = self.shell.render(window, cx, chrome, slots);
        // The window's own keys, handled above everything else: F6 moves between
        // the regions and Shift+F6 back, which is the Windows pair for reaching
        // what an editor would otherwise swallow along with Tab; Enter or Space
        // then runs whatever the focused region offers.
        //
        // The document keys are this platform's: Alt+Left and Alt+Right walk the
        // history the way a browser does, Ctrl+Tab cycles the open documents,
        // and Ctrl+W closes the one in front, which is what the tab strip's own
        // close button does.
        //
        // They are handled here rather than on the focused element itself
        // because a key event reaches an ancestor's listener, not the focused
        // element's own.
        div()
            .size_full()
            .on_key_down(cx.listener(|app, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "f6" if event.keystroke.modifiers.shift => app.cycle_focus(-1, window, cx),
                    "f6" => app.cycle_focus(1, window, cx),
                    "down" if app.list_focused(window) => app.move_in_list(Move::Step(1), cx),
                    "up" if app.list_focused(window) => app.move_in_list(Move::Step(-1), cx),
                    "home" if app.list_focused(window) => app.move_in_list(Move::First, cx),
                    "end" if app.list_focused(window) => app.move_in_list(Move::Last, cx),
                    "down" if app.rail_focus.is_focused(window) => app.move_in_rail(1, cx),
                    "up" if app.rail_focus.is_focused(window) => app.move_in_rail(-1, cx),
                    "enter" | "space" if app.actions_focus.is_focused(window) => {
                        app.run_primary_action(window, cx)
                    }
                    "left" if event.keystroke.modifiers.alt => app.go_back(window, cx),
                    "right" if event.keystroke.modifiers.alt => app.go_forward(window, cx),
                    "tab" if event.keystroke.modifiers.control => {
                        let step = if event.keystroke.modifiers.shift {
                            -1
                        } else {
                            1
                        };
                        app.cycle_tab(step, window, cx)
                    }
                    "w" if event.keystroke.modifiers.control => app.close_active_tab(window, cx),

                    _ => {}
                }
            }))
            .child(shell)
            // A dialog is drawn by Root's own layer, and the framework leaves it
            // out of the view tree on purpose: an application adds it where the
            // dialog should sit, which is above everything else here.
            .children(Root::render_dialog_layer(window, cx))
            .into_any_element()
    }
}

/// What the form's background work produces.
enum Outcome {
    /// The Server has never been configured and needs the setup fields.
    NeedsSetup {
        setup_code_configured: bool,
        oidc_configured: bool,
        /// What a previous setup attempt already staged there.
        staged: Option<StagedSetup>,
    },
    SignedIn,
}

impl DesktopApp {
    /// The form's primary action, branching the way the macOS model does: a
    /// Server that reports itself unconfigured reveals the setup fields, and a
    /// configured one goes straight to the browser.
    pub fn continue_from_form(&mut self, cx: &mut Context<Self>) {
        if self.sign_in.busy {
            return;
        }
        let values = self.sign_in.values(cx);
        if values.server_origin.is_empty() {
            self.sign_in.error = Some("Enter the Server address.".to_owned());
            cx.notify();
            return;
        }

        if self.sign_in.shows_setup {
            if values.setup_code.is_empty() {
                self.sign_in.error =
                    Some("Enter the setup code from the Server deployment.".to_owned());
                cx.notify();
                return;
            }
            if values.organization.is_empty() {
                self.sign_in.error = Some("Enter an organization name.".to_owned());
                cx.notify();
                return;
            }
            if values.default_project.is_empty() {
                self.sign_in.error = Some("Enter a default project name.".to_owned());
                cx.notify();
                return;
            }
            self.begin("Saving the Server configuration…", cx, move || {
                let origin = normalize_origin(&values.server_origin)?;
                let session = crate::sign_in::complete_setup(
                    &origin,
                    &values.setup_code,
                    &values.organization,
                    &values.default_project,
                    &values.allowed_domains,
                )?;
                install(&origin, session)
            });
        } else {
            self.begin("Asking the Server…", cx, move || {
                let origin = normalize_origin(&values.server_origin)?;
                let status = crate::sign_in::setup_status(&origin)?;
                if crate::sign_in::needs_setup(&status) {
                    return Ok(Outcome::NeedsSetup {
                        setup_code_configured: status.setup_code_configured,
                        oidc_configured: status.oidc_configured,
                        staged: staged_setup(&status),
                    });
                }
                if !status.oidc_configured {
                    return Err(
                        "Configure the Server's OIDC deployment settings before continuing."
                            .to_owned(),
                    );
                }
                let session = crate::sign_in::authenticate(&origin)?;
                install(&origin, session)
            });
        }
    }

    /// Back to the Server step, which is what the macOS client calls
    /// "Use a different Server".
    pub fn choose_another_server(&mut self, cx: &mut Context<Self>) {
        if self.sign_in.busy {
            return;
        }
        self.sign_in.shows_setup = false;
        self.sign_in.error = None;
        cx.notify();
    }

    /// Runs the network and browser work off the UI thread, because the browser
    /// step waits for a person.
    fn begin(
        &mut self,
        stage: &str,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> Result<Outcome, String> + Send + 'static,
    ) {
        self.sign_in.busy = true;
        self.sign_in.error = None;
        self.sign_in.stage = Some(stage.to_owned());
        cx.notify();
        let task = cx.background_executor().spawn(async move { work() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| this.finish(result, cx));
        })
        .detach();
    }

    fn finish(&mut self, result: Result<Outcome, String>, cx: &mut Context<Self>) {
        self.sign_in.busy = false;
        self.sign_in.stage = None;
        match result {
            Ok(Outcome::NeedsSetup {
                setup_code_configured,
                oidc_configured,
                staged,
            }) => {
                self.sign_in.shows_setup = true;
                self.sign_in.setup_code_configured = setup_code_configured;
                self.sign_in.oidc_configured = oidc_configured;
                self.sign_in.staged = staged;
            }
            Ok(Outcome::SignedIn) => {
                self.signed_in = true;
                self.reload(cx);
            }
            Err(error) => self.sign_in.error = Some(error),
        }
        cx.notify();
    }
}

/// The first-run settings the Server already holds, in the form's terms.
fn staged_setup(status: &crate::sign_in::SetupStatus) -> Option<StagedSetup> {
    let configuration = status.session.as_ref()?.configuration.as_ref()?;
    Some(StagedSetup {
        organization: configuration.org_name.clone(),
        default_project: configuration.default_project_name.clone(),
        allowed_domains: configuration.allowed_email_domains.join(", "),
    })
}

/// Hands the session to the daemon, which is the only party that keeps it.
fn install(origin: &str, session: crate::sign_in::Session) -> Result<Outcome, String> {
    engine::install_session(
        origin,
        &session.access_token,
        session.refresh_token.as_deref(),
    )?;
    Ok(Outcome::SignedIn)
}

/// The macOS client validates a Server origin the same way: HTTPS anywhere, HTTP
/// only on loopback.
fn normalize_origin(input: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(input.trim())
        .map_err(|_| "That is not a Server address.".to_owned())?;
    let host = url.host_str().unwrap_or_default().to_owned();
    match url.scheme() {
        "https" => {}
        "http" if host == "127.0.0.1" || host == "localhost" || host == "[::1]" => {}
        "http" => return Err("Remote Servers require HTTPS.".to_owned()),
        other => return Err(format!("A Server address starts with https, not {other}.")),
    }
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Ok(format!("{}://{host}{port}", url.scheme()))
}
