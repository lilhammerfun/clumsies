//! The Memory screen: the Project's file tree beside the selected document.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tree::TreeState;
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::{markdown, memory_tree};
use crate::engine::MemoryDocument;
use crate::ui::{self, Typography};

pub struct MemoryScreen {
    tree: Entity<TreeState>,
    documents: Vec<MemoryDocument>,
    /// Why the documents could not be read, when they could not be.
    error: Option<String>,
    /// Dropping a subscription cancels it, so the screen holds it.
    _selection: Subscription,
}

impl MemoryScreen {
    pub fn new(
        cx: &mut Context<DesktopApp>,
        documents: Vec<MemoryDocument>,
        error: Option<String>,
    ) -> Self {
        let tree = cx.new(|cx| {
            let mut state = TreeState::new(cx).items(memory_tree::items(&documents));
            select_first(&mut state, &documents, cx);
            state
        });
        // Selecting an entry notifies the tree state, not this view.
        let selection = cx.observe(&tree, |_, _, cx| cx.notify());
        Self {
            tree,
            documents,
            error,
            _selection: selection,
        }
    }

    /// Replaces the tree when the selected Project changes.
    pub fn set_documents(
        &mut self,
        documents: Vec<MemoryDocument>,
        error: Option<String>,
        cx: &mut Context<DesktopApp>,
    ) {
        self.documents = documents;
        self.error = error;
        let items = memory_tree::items(&self.documents);
        let documents = &self.documents;
        self.tree.update(cx, |state, cx| {
            state.set_items(items, cx);
            select_first(state, documents, cx);
        });
    }

    pub fn render(&self, cx: &mut Context<DesktopApp>) -> impl IntoElement {
        let selected = self
            .tree
            .read(cx)
            .selected_entry()
            .map(|entry| entry.item().id.to_string());

        let document = selected
            .as_deref()
            .and_then(|path| self.documents.iter().find(|document| document.path == path));

        let body: AnyElement = match (document, &self.error) {
            (Some(document), _) => {
                markdown::memory_document("memory-preview", document.content.clone())
                    .into_any_element()
            }
            (None, Some(error)) => ui::message(error.clone(), cx.theme().danger),
            (None, None) => ui::message("这个项目还没有 Memory。", cx.theme().muted_foreground),
        };

        let column = div()
            .v_flex()
            .w(px(230.))
            .h_full()
            .p_2()
            .gap_1()
            .child(section("Memory", cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .child(memory_tree::memory_tree(&self.tree)),
            );

        // h_flex centers the cross axis, so a column in a row takes its content
        // height unless it asks for h_full(); the scroll region inside needs the
        // row's height to resolve against.
        let preview = div()
            .v_flex()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .p_4()
            .gap_2()
            .child(section(selected.as_deref().unwrap_or_default(), cx))
            .child(div().flex_1().min_h(px(0.)).child(body));

        div()
            .h_flex()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .child(column)
            .child(preview)
    }
}

fn select_first(state: &mut TreeState, documents: &[MemoryDocument], cx: &mut Context<TreeState>) {
    let Some(first) = documents.first() else {
        return;
    };
    let id: SharedString = first.path.clone().into();
    state.set_selected_index(state.index_of(&id), cx);
}

fn section(label: &str, cx: &mut Context<DesktopApp>) -> impl IntoElement {
    div()
        .text_style(&ui::CAPTION)
        .text_color(cx.theme().muted_foreground)
        .child(label.to_owned())
}
