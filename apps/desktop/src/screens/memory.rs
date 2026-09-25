//! The Memory screen: the Project's file tree beside the selected document.
//!
//! A screen is a struct over its own state plus a render function. When one
//! grows state that has to outlive a frame, it becomes its own view entity; the
//! tree already is one.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tree::TreeState;
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::diff::{self, DiffPalette};
use crate::components::{markdown, memory_tree};
use crate::engine;
use crate::ui::{self, Typography};

pub struct MemoryScreen {
    tree: Entity<TreeState>,
    /// Dropping a subscription cancels it, so the screen holds it.
    _selection: Subscription,
}

impl MemoryScreen {
    pub fn new(cx: &mut Context<DesktopApp>) -> Self {
        let tree = cx.new(|cx| {
            let mut state = TreeState::new(cx).items(engine::memory_tree());
            let id: SharedString = "knowledge/README.md".into();
            state.set_selected_index(state.index_of(&id), cx);
            state
        });
        // Selecting an entry notifies the tree state, not this view.
        let selection = cx.observe(&tree, |_, _, cx| cx.notify());
        Self {
            tree,
            _selection: selection,
        }
    }

    pub fn render(&self, cx: &mut Context<DesktopApp>) -> impl IntoElement {
        let selected = self
            .tree
            .read(cx)
            .selected_entry()
            .map(|entry| entry.item().id.to_string());

        let body: AnyElement = match selected.as_deref().and_then(engine::draft) {
            Some(draft) => {
                let rows = diff::diff_rows(draft.before, draft.after);
                diff::diff_view(
                    rows,
                    cx.theme().mono_font_family.clone(),
                    DiffPalette::from_theme(cx.theme()),
                )
                .into_any_element()
            }
            None => {
                let text = selected
                    .as_deref()
                    .and_then(engine::document)
                    .map_or("在左侧选择一篇文档。", |document| {
                        document.content
                    });
                markdown::memory_document("memory-preview", text).into_any_element()
            }
        };

        let column = div()
            .v_flex()
            .w(px(230.))
            .h_full()
            .p_2()
            .gap_1()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child("Memory"),
            )
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
            .min_h(px(0.))
            .p_4()
            .gap_2()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child(selected.clone().unwrap_or_default()),
            )
            .child(div().flex_1().min_h(px(0.)).child(body));

        // The screen fills what the rail leaves; without flex_1 it would size to
        // its content and squeeze the preview into a strip.
        div()
            .h_flex()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .child(column)
            .child(preview)
    }
}
