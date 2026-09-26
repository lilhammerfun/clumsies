//! The application shell: the Project rail beside the active screen.

use clumsiesd::{DaemonDraftOperationResponse, DaemonDraftSummary};
use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::engine::{self, Checkout, DocumentEdit, EngineStatus, Project, Review};
use crate::screens::document::{Mode, Notice, SAVE_DELAY, SaveState};
use crate::screens::memory::MemoryScreen;
use crate::screens::sign_in::{SignInScreen, StagedSetup};
use crate::ui::{self, Typography};

/// Width of the Project rail.
const RAIL_WIDTH: f32 = 200.;

pub struct DesktopApp {
    /// The engine is asked when the window opens and again when its row is
    /// clicked. Asking inside a frame would stall the render on a socket read.
    engine: EngineStatus,
    projects: Vec<Project>,
    /// Why the Project list could not be read, when it could not be.
    projects_error: Option<String>,
    selected_project: Option<usize>,
    memory: MemoryScreen,
    /// The form shown while the daemon has no Server session.
    sign_in: SignInScreen,
    signed_in: bool,
    /// Which debounced store owns the editor. A store that a later keystroke
    /// has superseded must not report its result as the editor's state.
    save_generation: u64,
    /// Debug-build probe for the platform input method. Not part of the
    /// product: DESIGN.md keeps development scaffolding out of shipped UI.
    probe: Entity<InputState>,
}

impl Default for DesktopApp {
    fn default() -> Self {
        unreachable!("DesktopApp is built with a window")
    }
}

impl DesktopApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = engine::engine_status();
        let (projects, projects_error) = read_projects();
        let (checkout, checkout_error) = match projects.first() {
            Some(project) => read_checkout(&project.project_id),
            None => (None, None),
        };
        let memory = MemoryScreen::new(window, cx, checkout, checkout_error);
        let probe = cx.new(|cx| InputState::new(window, cx).placeholder("用中文输入法打几个字"));
        // A daemon with no session refuses every Server request, and that
        // refusal is the only signed-out signal there is.
        let signed_in = projects_error
            .as_deref()
            .is_none_or(|error| !engine::missing_session(error));
        let server_url = engine::configured_server_url().unwrap_or_default();
        let sign_in = SignInScreen::new(window, cx, &server_url);
        let mut app = Self {
            engine,
            selected_project: signed_in.then_some(0),
            projects,
            projects_error,
            memory,
            sign_in,
            signed_in,
            save_generation: 0,
            probe,
        };
        // A Project that already holds a proposal must show it on the first
        // frame: the tree marks it and the pane offers to review it.
        app.refresh_drafts(cx);
        app
    }

    /// The Memory screen, which the tree reaches through its own selection
    /// notification.
    pub fn memory(&mut self) -> &mut MemoryScreen {
        &mut self.memory
    }

    /// Selecting a Project reads its Memory. That read is a socket call to the
    /// daemon, which is why it happens on the click rather than every frame.
    fn select_project(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_project = Some(index);
        if let Some(project) = self.projects.get(index) {
            let (checkout, error) = read_checkout(&project.project_id);
            self.memory.set_checkout(checkout, error, cx);
            self.refresh_drafts(cx);
        }
        cx.notify();
    }

    /// A keystroke landed in the editor. The store waits for a pause in typing,
    /// which is what the macOS client's 600ms debounce is for: every store is a
    /// socket call into the daemon and an upload behind it.
    ///
    /// The edit is captured now rather than when the pause ends, because by
    /// then the reader may have opened another document or another Project, and
    /// this text belongs to the one it was typed in.
    pub fn document_edited(&mut self, cx: &mut Context<Self>) {
        self.memory.pane_mut().set_save_state(SaveState::Pending);
        self.save_generation += 1;
        let generation = self.save_generation;
        let Some(edit) = self.document_edit(cx) else {
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
        if generation != self.save_generation {
            return;
        }
        let opens_the_written_document = self
            .memory
            .selected_document()
            .is_some_and(|document| document.resource_id == edit.resource_id);
        if opens_the_written_document {
            self.memory.pane_mut().set_save_state(SaveState::Saving);
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

    /// What one store produced. The text is recorded as stored even when a
    /// later keystroke has already asked for another store, because that is
    /// what "unsaved" is measured against; only the newest store of the open
    /// document decides what the pane reports.
    fn document_stored(
        &mut self,
        generation: u64,
        resource_id: &str,
        content: &str,
        result: Result<DaemonDraftOperationResponse, String>,
        cx: &mut Context<Self>,
    ) {
        // A store that landed may have created or advanced a draft, and the
        // tree marks every document whose draft moved, so the list is re-read
        // whichever document the reader is now looking at.
        let stored = match &result {
            Ok(response) => Some(response.draft_id.clone()),
            Err(_) => None,
        };
        if let Some(draft_id) = stored {
            self.refresh_drafts(cx);
            self.follow_upload(draft_id, cx);
        }
        // The rest is the pane's, and a store may finish after the reader has
        // opened another document. The engine has the text either way; the pane
        // only speaks for what it is showing.
        if !self
            .memory
            .selected_document()
            .is_some_and(|document| document.resource_id == resource_id)
        {
            return;
        }
        self.memory.pane_mut().accept_text(content.to_owned());
        if generation == self.save_generation {
            match result {
                Ok(_) => self.memory.pane_mut().set_save_state(SaveState::Saved),
                Err(error) => self
                    .memory
                    .pane_mut()
                    .set_save_state(SaveState::Failed(error)),
            }
        }
        cx.notify();
    }

    /// Waits for the daemon to upload a just-stored draft and re-reads the
    /// list, so the pane stops saying "uploading" the moment that stops being
    /// true. macOS refreshes the draft after a store for the same reason; the
    /// daemon pushes no event here, so the client asks once.
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

    pub fn set_document_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.memory.pane_mut().set_mode(mode);
        cx.notify();
    }

    /// A Review was created for the draft the sheet held. The macOS client
    /// switches to its Reviews section here; this client reports the Review and
    /// leaves the reader in the document, because there is no Reviews screen
    /// yet.
    pub fn review_requested(&mut self, review: Review, cx: &mut Context<Self>) {
        self.memory.set_notice(Some(Notice {
            text: format!(
                "Review {} requested · {}",
                tail_of(&review.review_id),
                review.title
            ),
        }));
        self.refresh_drafts(cx);
        cx.notify();
    }

    /// The edit the pane would store: its text, the document it belongs to, and
    /// the draft that already carries it when there is one.
    fn document_edit(&self, cx: &App) -> Option<DocumentEdit> {
        let project = self.projects.get(self.selected_project?)?;
        let document = self.memory.selected_document()?;
        let draft = self.memory.selected_draft();
        Some(DocumentEdit {
            project_id: project.project_id.clone(),
            base_commit_id: draft
                .and_then(|draft| draft.base_commit_id.clone())
                .or_else(|| self.memory.commit_id().map(str::to_owned)),
            draft_id: draft.map(|draft| draft.draft_id.clone()),
            resource_id: document.resource_id.clone(),
            content: self.memory.pane().text(cx),
        })
    }

    /// Re-reads the drafts of the selected Project. The daemon is asked in the
    /// background because the answer is a socket call, and a draft list that
    /// could not be read is not a failed edit: the rail already reports whether
    /// the engine answers at all.
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
        if let Ok(drafts) = result {
            self.memory.set_drafts(drafts, cx);
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
        self.projects = projects;
        self.projects_error = projects_error;
        self.memory.set_checkout(checkout, checkout_error, cx);
        self.refresh_drafts(cx);
    }

    /// Returns an owned element on purpose: edition 2024 makes `impl Trait` capture every input lifetime, so returning `impl IntoElement` here would
    /// hold the borrow of `cx` for the whole render.
    fn rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected_project;
        let typed = self.probe.read(cx).value();

        let rows = self
            .projects
            .iter()
            .enumerate()
            .map(|(index, project)| {
                let row = div()
                    .id(("project", index))
                    .px_2()
                    .py_1()
                    .rounded(px(ui::RADIUS))
                    .text_style(&ui::BODY)
                    .child(project.name.clone());
                let row = if Some(index) == selected {
                    row.bg(cx.theme().list_active)
                } else {
                    row
                };
                row.on_click(cx.listener(move |this, _event, _window, cx| {
                    this.select_project(index, cx);
                }))
            })
            .collect::<Vec<_>>();

        // An empty account, an unreachable engine and an empty organization are
        // three different situations and the rail says which one it is in.
        let projects: AnyElement = match &self.projects_error {
            Some(error) => ui::message(error.clone(), cx.theme().danger),
            None if self.projects.is_empty() => {
                ui::message("No Projects yet.", cx.theme().muted_foreground)
            }
            None => div().v_flex().gap_1().children(rows).into_any_element(),
        };

        let (engine_state, engine_state_color) = match &self.engine {
            EngineStatus::Connected(health) => (
                format!("v{} · connected", health.daemon_version),
                cx.theme().success,
            ),
            EngineStatus::Unreachable(_) => {
                ("unavailable · click to retry".to_owned(), cx.theme().danger)
            }
        };
        let engine_detail = match &self.engine {
            EngineStatus::Connected(health) => format!(
                "{} · {}",
                host_of(&health.server_url),
                if health.project_id.is_some() {
                    "project bound"
                } else {
                    "signed in"
                },
            ),
            EngineStatus::Unreachable(reason) => reason.clone(),
        };
        let engine = div()
            .v_flex()
            .gap_1()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child("Engine"),
            )
            .child(
                div()
                    .id("engine-status")
                    .px_2()
                    .py_1()
                    .rounded(px(ui::RADIUS))
                    .text_style(&ui::CAPTION)
                    .text_color(engine_state_color)
                    .child(engine_state)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.engine = engine::engine_status();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child(truncate(&engine_detail, 30)),
            )
            .children(match &self.engine {
                EngineStatus::Connected(health) => Some(
                    div()
                        .text_style(&ui::CAPTION)
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "install {} · schema {}",
                            short_installation(&health.daemon_installation_id),
                            health.local_db.schema_version
                        ))
                        .into_any_element(),
                ),
                EngineStatus::Unreachable(_) => None,
            });

        let probe = cfg!(debug_assertions).then(|| {
            div()
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .text_style(&ui::CAPTION)
                        .text_color(cx.theme().muted_foreground)
                        .child("Input method probe (debug builds only)"),
                )
                .child(Input::new(&self.probe))
                .child(
                    div()
                        .text_style(&ui::CAPTION)
                        .child(format!("你输入的是：{typed}")),
                )
                .into_any_element()
        });

        div()
            .v_flex()
            .w(px(RAIL_WIDTH))
            .h_full()
            .p_3()
            .gap_1()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child("Projects"),
            )
            .child(projects)
            .child(div().flex_1())
            .children(probe)
            .child(engine)
            .into_any_element()
    }
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
        // window rather than sitting beside an empty rail.
        if !self.signed_in {
            // A staged configuration arrives without a window, so the form
            // takes it here, before it draws with its fields.
            self.sign_in.apply_staged(window, cx);
            return self.sign_in.render(cx);
        }
        // A tree click arrives as a notification, which carries no window, so
        // the editor takes its new text at the top of a frame, before the pane
        // draws with it.
        self.memory.apply_pending_load(window, cx);
        let rail = self.rail(cx);
        let screen = self.memory.render(cx);
        div()
            .h_flex()
            .size_full()
            .child(rail)
            .child(screen)
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

/// `https://app.clumsies.ai` is the host the reader recognises.
fn host_of(server_url: &str) -> &str {
    server_url
        .split_once("://")
        .map_or(server_url, |(_, host)| host)
}

/// `daemon_3a4e923421294eab8bc84065ff210644` reads better as its tail.
fn short_installation(id: &str) -> String {
    match id.rsplit_once('_') {
        Some((_, tail)) if tail.len() > 6 => format!("…{}", &tail[tail.len() - 6..]),
        _ => id.to_owned(),
    }
}

/// A Server identity is long and its tail is what tells two apart.
fn tail_of(id: &str) -> String {
    match id.rsplit_once('_') {
        Some((_, tail)) if tail.len() > 6 => format!("…{}", &tail[tail.len() - 6..]),
        _ => id.to_owned(),
    }
}

/// Paths and OS errors are long; a rail line is not.
fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let kept: String = value.chars().take(limit.saturating_sub(1)).collect();
    format!("{kept}…")
}
