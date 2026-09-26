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

use crate::components::file_tree::{self, Decoration, PathEntry};
use crate::engine::MemoryDocument;

/// The Project's documents, as the tree's entries.
pub fn items(documents: &[MemoryDocument]) -> Vec<TreeItem> {
    let entries: Vec<PathEntry> = documents
        .iter()
        .map(|document| PathEntry::new(document.path.clone()))
        .collect();
    file_tree::items(&entries)
}

/// The tree, with Memory's decoration on every file an open draft touches.
pub fn memory_tree(
    state: &Entity<TreeState>,
    drafted: &BTreeSet<String>,
    menu: impl Fn(&str, PopupMenu, &mut Window, &mut App) -> PopupMenu + 'static,
) -> impl IntoElement {
    let drafted = drafted.clone();
    file_tree::path_tree(
        state,
        move |path| Decoration {
            tone: None,
            label: drafted.contains(path).then_some("draft"),
        },
        menu,
    )
}
