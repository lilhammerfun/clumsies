//! Clumsies desktop client for Windows and Linux.
//!
//! Everything here is a development skeleton: hard-coded data, no engine calls.
//! It proves the pieces the real client depends on -- window, layout, list and
//! tree interaction, platform input methods, and Markdown rendering.

use gpui_kit::base::StyledExt;
use gpui_kit::component::Root;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::text::{FrontmatterPlugin, MarkdownExtensions, TextView};
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::*;

struct MemoryDocument {
    /// Stable id, also the path shown above the preview.
    path: &'static str,
    content: &'static str,
}

/// A stand-in for the Project's Effective Memory. The engine hands us these
/// documents for real once the client is wired to it.
const MEMORY_DOCUMENTS: [MemoryDocument; 5] = [
    MemoryDocument {
        path: "knowledge/README.md",
        content: include_str!("../../macos/Resources/MemoryStarter/knowledge/README.md"),
    },
    MemoryDocument {
        path: "lessons/README.md",
        content: include_str!("../../macos/Resources/MemoryStarter/lessons/README.md"),
    },
    MemoryDocument {
        path: "procedures/README.md",
        content: include_str!("../../macos/Resources/MemoryStarter/procedures/README.md"),
    },
    MemoryDocument {
        path: "procedures/rollback.md",
        content: include_str!("../assets/markdown-sample.md"),
    },
    MemoryDocument {
        path: "skills/project-memory/SKILL.md",
        content: include_str!("../../../packages/clumsies/skills/project-memory/SKILL.md"),
    },
];

fn memory_document(path: &str) -> Option<&'static MemoryDocument> {
    MEMORY_DOCUMENTS
        .iter()
        .find(|document| document.path == path)
}

/// The tree the engine will eventually build from the Project's Memory refs.
fn memory_tree_items() -> Vec<TreeItem> {
    let file = |path: &'static str, label: &'static str| TreeItem::new(path, label);
    vec![
        TreeItem::new("knowledge", "knowledge")
            .expanded(true)
            .child(file("knowledge/README.md", "README.md")),
        TreeItem::new("lessons", "lessons")
            .expanded(true)
            .child(file("lessons/README.md", "README.md")),
        TreeItem::new("procedures", "procedures")
            .expanded(true)
            .child(file("procedures/README.md", "README.md"))
            .child(file("procedures/rollback.md", "部署回滚清单.md")),
        TreeItem::new("skills", "skills").expanded(true).child(
            TreeItem::new("skills/project-memory", "project-memory")
                .expanded(true)
                .child(file("skills/project-memory/SKILL.md", "SKILL.md")),
        ),
    ]
}

struct Project {
    name: &'static str,
    repository: &'static str,
    memory_count: usize,
}

struct DesktopApp {
    projects: Vec<Project>,
    selected_project: usize,
    memory_tree: Entity<TreeState>,
    /// Dropping a subscription cancels it, so the view has to hold it.
    _tree_selection: Subscription,
    /// Input method probe: the same text input the Memory editor will use.
    probe: Entity<InputState>,
}

impl DesktopApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let memory_tree = cx.new(|cx| {
            let mut state = TreeState::new(cx).items(memory_tree_items());
            let id: SharedString = "knowledge/README.md".into();
            state.set_selected_index(state.index_of(&id), cx);
            state
        });
        // Selecting an entry notifies the tree state, not this view.
        let tree_selection = cx.observe(&memory_tree, |_, _, cx| cx.notify());

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
            selected_project: 0,
            memory_tree,
            _tree_selection: tree_selection,
            probe,
        }
    }
}

impl Render for DesktopApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_project = self.selected_project;
        let typed = self.probe.read(cx).value();

        let selected_path = self
            .memory_tree
            .read(cx)
            .selected_entry()
            .map(|entry| entry.item().id.to_string());
        let document = selected_path.as_deref().and_then(memory_document);

        let projects = div()
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
                        let row = if index == selected_project {
                            row.bg(rgb(0x2f3542))
                        } else {
                            row
                        };
                        row.on_click(cx.listener(move |this, _event, _window, cx| {
                            this.selected_project = index;
                            cx.notify();
                        }))
                    })
                    .collect::<Vec<_>>(),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .child(self.projects[selected_project].repository),
            )
            .child(div().text_sm().child(format!(
                "{} Memory",
                self.projects[selected_project].memory_count
            )))
            .child(div().mt_2().text_sm().child("Input method probe"))
            .child(Input::new(&self.probe))
            .child(div().text_sm().child(format!("你输入的是：{typed}")));

        let tree_view = tree(
            &self.memory_tree,
            |index, entry, _selected, _window, _cx| {
                let marker = if entry.is_folder() {
                    if entry.is_expanded() { "▾ " } else { "▸ " }
                } else {
                    "   "
                };
                ListItem::new(index).child(
                    div()
                        .pl(px(entry.depth() as f32 * 14.))
                        .child(format!("{marker}{}", entry.item().label)),
                )
            },
        );

        let memory_column = div()
            .v_flex()
            .w(px(230.))
            .h_full()
            .p_2()
            .gap_1()
            .child(div().text_sm().child("Memory"))
            .child(div().flex_1().min_h(px(0.)).child(tree_view));

        // Frontmatter is not part of CommonMark, so the parser has to be told
        // to read it and a plugin has to render the resulting node.
        let preview_text = document.map_or("在左侧选择一篇文档。", |document| {
            document.content
        });
        // h_flex centers the cross axis, so a column in a row takes its content
        // height unless it asks for h_full(); the scroll region inside needs the
        // row's height to resolve against.
        let preview = div()
            .v_flex()
            .flex_1()
            .h_full()
            .min_h(px(0.))
            .p_4()
            .gap_2()
            .child(div().text_sm().child(selected_path.unwrap_or_default()))
            .child(
                div().flex_1().min_h(px(0.)).child(
                    TextView::markdown("memory-preview", preview_text)
                        .markdown_extensions(MarkdownExtensions::default().frontmatter())
                        .plugin(FrontmatterPlugin::new())
                        .selectable(true)
                        .scrollable(true)
                        .size_full(),
                ),
            );

        div()
            .h_flex()
            .size_full()
            .child(projects)
            .child(memory_column)
            .child(preview)
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        // The app id groups the window under one desktop entry; the title is
        // what the window list and the compositor show.
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Clumsies".into()),
                ..Default::default()
            }),
            app_id: Some("ai.clumsies.desktop".into()),
            ..Default::default()
        };
        cx.spawn(async move |cx| {
            cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| DesktopApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        })
        .detach();
    });
}
