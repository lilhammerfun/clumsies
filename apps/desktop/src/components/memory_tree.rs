//! Memory's view of the file tree.
//!
//! The tree itself is `components::file_tree`, which knows about paths and
//! nothing else; this is the layer that knows a Project keeps Memory in files,
//! that an open draft marks some of them, and that a row's menu has Memory
//! commands after the generic ones.

use std::collections::BTreeSet;

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::tree::{TreeItem, TreeState};
use gpui_kit::*;

use crate::components::file_tree::{self, Decoration, PathEntry, RowClick};

/// The paths a screen wants drawn, as the tree's entries. A screen filters the
/// Project's documents itself — the tree knows nothing about a filter — hands
/// over the paths that survived, and says which folders it has folded.
pub fn items(paths: &[String], folded: &BTreeSet<String>) -> Vec<TreeItem> {
    let entries: Vec<PathEntry> = paths.iter().cloned().map(PathEntry::new).collect();
    file_tree::items(&entries, folded)
}

/// The tree, with Memory's decoration on every file an open draft touches:
/// `draft` for a document a proposal rewrites, `new` for one it would create.
pub fn memory_tree(
    state: &Entity<TreeState>,
    selection: &file_tree::Selection,
    drafted: &BTreeSet<String>,
    proposed: &BTreeSet<String>,
    on_click: impl Fn(RowClick, &mut Window, &mut App) + 'static,
    menu: impl Fn(&str, PopupMenu, &mut Window, &mut App) -> PopupMenu + 'static,
) -> impl IntoElement {
    let drafted = drafted.clone();
    let proposed = proposed.clone();
    file_tree::path_tree(
        state,
        selection,
        move |path| Decoration {
            tone: None,
            label: if proposed.contains(path) {
                Some("new")
            } else if drafted.contains(path) {
                Some("draft")
            } else {
                None
            },
        },
        on_click,
        menu,
    )
}
