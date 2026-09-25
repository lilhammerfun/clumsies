//! Clumsies desktop client for Windows and Linux.
//!
//! Everything here is a development skeleton: hard-coded data, no engine calls.
//! It proves the pieces the real client depends on -- window, layout, list
//! interaction, platform input methods, and Markdown rendering.

use gpui_kit::base::StyledExt;
use gpui_kit::component::Root;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::text::{FrontmatterPlugin, MarkdownExtensions, TextView};
use gpui_kit::*;

/// A real product document, rendered straight out of the repository.
const REAL_DOCUMENT: &str =
    include_str!("../../../packages/clumsies/skills/project-memory/SKILL.md");

/// A document that exercises every block the Memory editor has to render.
const SAMPLE_DOCUMENT: &str = include_str!("../assets/markdown-sample.md");

const DOCUMENTS: [(&str, &str); 2] = [
    ("真实文档 SKILL.md", REAL_DOCUMENT),
    ("渲染压力测试", SAMPLE_DOCUMENT),
];

struct Project {
    name: &'static str,
    repository: &'static str,
    memory_count: usize,
}

struct DesktopApp {
    projects: Vec<Project>,
    selected: usize,
    /// Input method probe: the same text input the Memory editor will use.
    probe: Entity<InputState>,
    selected_document: usize,
}

impl DesktopApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let probe = cx.new(|cx| InputState::new(window, cx).placeholder("用中文输入法打几个字"));
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
            probe,
            selected_document: 0,
        }
    }
}

impl Render for DesktopApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_index = self.selected;
        let selected_document = self.selected_document;
        let typed = self.probe.read(cx).value();

        let sidebar = div()
            .v_flex()
            .w(px(200.))
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

        let tabs = div().h_flex().gap_2().children(
            DOCUMENTS
                .iter()
                .enumerate()
                .map(|(index, (label, _))| {
                    let tab = div()
                        .id(("document", index))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .child(*label);
                    let tab = if index == selected_document {
                        tab.bg(rgb(0x2f3542))
                    } else {
                        tab
                    };
                    tab.on_click(cx.listener(move |this, _event, _window, cx| {
                        this.selected_document = index;
                        cx.notify();
                    }))
                })
                .collect::<Vec<_>>(),
        );

        // Frontmatter is not part of CommonMark, so the parser has to be told
        // to read it and a plugin has to render the resulting node.
        let preview = div().flex_1().min_h(px(0.)).child(
            TextView::markdown("memory-preview", DOCUMENTS[selected_document].1)
                .markdown_extensions(MarkdownExtensions::default().frontmatter())
                .plugin(FrontmatterPlugin::new())
                .selectable(true)
                .scrollable(true)
                .size_full(),
        );

        let project = &self.projects[selected_index];
        let detail = div()
            .v_flex()
            .flex_1()
            .min_h(px(0.))
            .p_4()
            .gap_2()
            .child(div().text_lg().child(project.name))
            .child(format!("Repository: {}", project.repository))
            .child(format!("Selected Memory: {}", project.memory_count))
            .child(div().mt_2().text_sm().child("Input method probe"))
            .child(Input::new(&self.probe))
            .child(format!("你输入的是：{typed}"))
            .child(div().mt_2().text_sm().child("Markdown preview"))
            .child(tabs)
            .child(preview);

        div().h_flex().size_full().child(sidebar).child(detail)
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|cx| DesktopApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        })
        .detach();
    });
}
