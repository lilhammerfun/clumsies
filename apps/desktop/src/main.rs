//! Clumsies desktop client for Windows and Linux.
//!
//! The data below is hard-coded on purpose: this first version only proves that
//! GPUI can open a window, lay out columns, render a list and react to clicks.
//! Reading real Projects and Memory from `clumsiesd` comes next.

use gpui_kit::base::StyledExt;
use gpui_kit::component::Root;
use gpui_kit::*;

struct Project {
    name: &'static str,
    repository: &'static str,
    memory_count: usize,
}

struct DesktopApp {
    projects: Vec<Project>,
    selected: usize,
}

impl DesktopApp {
    fn new() -> Self {
        Self {
            projects: vec![
                Project {
                    name: "clumsies",
                    repository: "~/Projects/clumsies",
                    memory_count: 12,
                },
                Project {
                    name: "atlas-api",
                    repository: "~/Projects/atlas-api",
                    memory_count: 7,
                },
                Project {
                    name: "web-console",
                    repository: "~/Projects/web-console",
                    memory_count: 0,
                },
            ],
            selected: 0,
        }
    }
}

impl Render for DesktopApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_index = self.selected;

        let sidebar = div()
            .v_flex()
            .w(px(220.))
            .h_full()
            .p_3()
            .gap_1()
            .child(div().text_sm().child("Projects"))
            .children(
                self.projects
                    .iter()
                    .enumerate()
                    .map(|(index, project)| {
                        let row = div()
                            .id(("project", index))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .child(project.name);
                        let row = if index == selected_index {
                            row.bg(rgb(0x2f3542))
                        } else {
                            row
                        };
                        row.on_click(cx.listener(move |this, _event, _window, cx| {
                            this.selected = index;
                            cx.notify();
                        }))
                    })
                    .collect::<Vec<_>>(),
            );

        let project = &self.projects[selected_index];
        let detail = div()
            .v_flex()
            .flex_1()
            .p_4()
            .gap_2()
            .child(div().text_lg().child(project.name))
            .child(format!("Repository: {}", project.repository))
            .child(format!("Selected Memory: {}", project.memory_count));

        div().h_flex().size_full().child(sidebar).child(detail)
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|_| DesktopApp::new());
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        })
        .detach();
    });
}
