//! The Memory file tree.

use gpui_kit::base::StyledExt;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::{TreeState, tree};
use gpui_kit::*;

use crate::ui;

pub fn memory_tree(state: &Entity<TreeState>) -> impl IntoElement {
    tree(state, |index, entry, _selected, _window, _cx| {
        let marker = if entry.is_folder() {
            if entry.is_expanded() { "▾" } else { "▸" }
        } else {
            " "
        };
        ListItem::new(index).child(
            div()
                .h_flex()
                .gap_2()
                .pl(px(entry.depth() as f32 * ui::SPACE_MD))
                .child(div().w(px(ui::SPACE_MD)).child(marker))
                .child(entry.item().label.clone()),
        )
    })
}
