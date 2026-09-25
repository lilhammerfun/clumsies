//! The application shell: the Project rail beside the active screen.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::engine;
use crate::screens::memory::MemoryScreen;
use crate::ui::{self, Typography};

/// Width of the Project rail.
const RAIL_WIDTH: f32 = 200.;

pub struct DesktopApp {
    projects: Vec<engine::Project>,
    selected_project: usize,
    memory: MemoryScreen,
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
            .into_any_element()
    }
}

impl Render for DesktopApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rail = self.rail(cx);
        let screen = self.memory.render(cx);
        div().h_flex().size_full().child(rail).child(screen)
    }
}
