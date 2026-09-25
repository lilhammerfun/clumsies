//! The application shell: the Project rail beside the active screen.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::engine::{self, EngineStatus, MemoryDocument, Project};
use crate::screens::memory::MemoryScreen;
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
    /// Debug-build probe for the platform input method. Not part of the
    /// product: DESIGN.md keeps development scaffolding out of shipped UI.
    probe: Entity<InputState>,
}

impl DesktopApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = engine::engine_status();
        let (projects, projects_error) = match engine::projects() {
            Ok(projects) => (projects, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let (documents, memory_error) = match projects.first() {
            Some(project) => read_memory(project),
            None => (Vec::new(), None),
        };
        let memory = MemoryScreen::new(cx, documents, memory_error);
        let probe = cx.new(|cx| InputState::new(window, cx).placeholder("用中文输入法打几个字"));
        Self {
            engine,
            selected_project: (!projects.is_empty()).then_some(0),
            projects,
            projects_error,
            memory,
            probe,
        }
    }

    /// Selecting a Project reads its Memory. That read is a socket call to the
    /// daemon, which is why it happens on the click rather than every frame.
    fn select_project(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_project = Some(index);
        if let Some(project) = self.projects.get(index) {
            let (documents, error) = read_memory(project);
            self.memory.set_documents(documents, error, cx);
        }
        cx.notify();
    }

    /// Returns an owned element on purpose: edition 2024 makes `impl Trait`
    /// capture every input lifetime, so returning `impl IntoElement` here would
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
                ui::message("还没有项目。", cx.theme().muted_foreground)
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

/// Reads one Project's Memory, keeping the reason when it cannot.
fn read_memory(project: &Project) -> (Vec<MemoryDocument>, Option<String>) {
    match engine::memory_documents(&project.project_id) {
        Ok(documents) => (documents, None),
        Err(error) => (Vec::new(), Some(error)),
    }
}

impl Render for DesktopApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rail = self.rail(cx);
        let screen = self.memory.render(cx);
        div().h_flex().size_full().child(rail).child(screen)
    }
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

/// Paths and OS errors are long; a rail line is not.
fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let kept: String = value.chars().take(limit.saturating_sub(1)).collect();
    format!("{kept}…")
}
