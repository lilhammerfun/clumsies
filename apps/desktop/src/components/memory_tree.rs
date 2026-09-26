//! The Memory file tree, built from the Project's document paths.

use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::*;

use crate::engine::MemoryDocument;
use crate::ui::{self, Typography};

/// The daemon hands over a flat list of paths; a tree needs the directories
/// that the paths imply. Which of them an open draft touches is the row
/// renderer's business, not the walk's.
pub fn items(documents: &[MemoryDocument]) -> Vec<TreeItem> {
    let mut root = Directory::default();
    for document in documents {
        root.insert(&document.path);
    }
    root.into_items("")
}

#[derive(Default)]
struct Directory {
    directories: BTreeMap<String, Directory>,
    files: Vec<String>,
}

impl Directory {
    fn insert(&mut self, path: &str) {
        let mut parts = path.split('/').peekable();
        let mut node = self;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                node.files.push(part.to_owned());
            } else {
                node = node.directories.entry(part.to_owned()).or_default();
            }
        }
    }

    /// Directories first, then files; both alphabetical, because the walk is.
    /// An entry id is the full path, which is what the tree renders from.
    fn into_items(self, prefix: &str) -> Vec<TreeItem> {
        let mut items = Vec::new();
        for (name, directory) in self.directories {
            let path = join(prefix, &name);
            items.push(
                TreeItem::new(path.clone(), name)
                    .expanded(true)
                    .children(directory.into_items(&path)),
            );
        }
        for name in self.files {
            let path = join(prefix, &name);
            items.push(TreeItem::new(path, name));
        }
        items
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

/// The tree, with a Draft mark on every file an open draft touches.
pub fn memory_tree(state: &Entity<TreeState>, drafted: &BTreeSet<String>) -> impl IntoElement {
    let drafted = drafted.clone();
    tree(state, move |index, entry, _selected, _window, cx| {
        let marker = if entry.is_folder() {
            if entry.is_expanded() { "▾" } else { "▸" }
        } else {
            " "
        };
        let id = entry.item().id.to_string();
        let mark = drafted.contains(&id).then(|| {
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().primary)
                .child("draft")
        });
        ListItem::new(index).child(
            div()
                .h_flex()
                .gap_2()
                .pl(px(entry.depth() as f32 * ui::SPACE_MD))
                .child(div().w(px(ui::SPACE_MD)).child(marker))
                .child(entry.item().label.clone())
                .children(mark),
        )
    })
}
