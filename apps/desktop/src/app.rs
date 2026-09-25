//! The application shell: the Project rail beside the active screen.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::engine;
use crate::protocol::EngineStatus;
use crate::screens::memory::MemoryScreen;
use crate::ui::{self, Typography};

/// Width of the Project rail.
const RAIL_WIDTH: f32 = 200.;

pub struct DesktopApp {
    projects: Vec<engine::Project>,
    selected_project: usize,
    memory: MemoryScreen,
    /// The engine is asked when the window opens and again when its row is
    /// clicked. Asking inside a frame would stall the render on a socket read.
    engine: EngineStatus,
    /// Debug-build probe for the platform input method. Not part of the
    /// product: DESIGN.md keeps development scaffolding out of shipped UI.
    probe: Entity<InputState>,
}

impl DesktopApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let memory = MemoryScreen::new(cx);
        let probe = cx.new(|cx| InputState::new(window, cx).placeholder("用中文输入法打几个字"));
        Self {
            projects: engine::projects(),
            selected_project: 0,
            memory,
            engine: engine::engine_status(),
            probe,
        }
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
                    .child(project.name);
                let row = if index == selected {
                    row.bg(cx.theme().list_active)
                } else {
                    row
                };
                row.on_click(cx.listener(move |this, _event, _window, cx| {
                    this.selected_project = index;
                    cx.notify();
                }))
            })
            .collect::<Vec<_>>();

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

        // The engine is the one thing on this rail that is not a fixture, so
        // it says what it is: reachable, and what it reports about itself.
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
                    "signed out"
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

        let project = &self.projects[selected];
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
            .children(rows)
            .child(div().flex_1())
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child(project.repository),
            )
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} Memory", project.memory_count)),
            )
            .children(probe)
            .child(engine)
            .into_any_element()
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

impl Render for DesktopApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rail = self.rail(cx);
        let screen = self.memory.render(cx);
        div().h_flex().size_full().child(rail).child(screen)
    }
}
