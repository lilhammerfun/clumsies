//! A tree of paths, and nothing else.
//!
//! Read from the macOS client's `PathTreeView`: one generic tree serves the
//! Memory navigator and the review's changed-file list, and each of them layers
//! its own decoration and its own commands on top. The generic part is the
//! projection (a flat list of paths becomes directories and files), the row, and
//! the commands every view has — open, edit, rename, delete; everything a
//! particular screen knows about a file is handed in from outside.
//!
//! Nothing here knows what Memory is.

use std::collections::BTreeMap;

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::*;

use crate::ui::{self, Typography};

/// One file in a tree: what it is called, where it lives, and the identity a
/// screen uses to talk about it (a resource, a draft, a changed file).
#[derive(Clone, PartialEq, Eq)]
pub struct PathEntry {
    pub path: String,
}

impl PathEntry {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

/// What a screen adds to a row: the tone of its title, and a word for whatever
/// state it is in. A tree with no decoration at all is the default.
#[derive(Default)]
pub struct Decoration {
    pub tone: Option<Hsla>,
    pub label: Option<&'static str>,
}

/// The flat list of paths becomes the directories that imply it: directories
/// first, then files, both in the order they were given.
pub fn items(entries: &[PathEntry]) -> Vec<TreeItem> {
    let mut root = Directory::default();
    for entry in entries {
        root.insert(&entry.path);
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
            items.push(TreeItem::new(join(prefix, &name), name));
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

/// The tree, with a screen's decoration on every row and a screen's menu on
/// every row. The path is handed to both, because that is all a screen needs to
/// decide what a file looks like and what can be done with it.
pub fn path_tree(
    state: &Entity<TreeState>,
    decorate: impl Fn(&str) -> Decoration + 'static,
    build_menu: impl Fn(&str, PopupMenu, &mut Window, &mut App) -> PopupMenu + 'static,
) -> impl IntoElement {
    tree(state, move |index, entry, _selected, _window, cx| {
        let marker = if entry.is_folder() {
            if entry.is_expanded() { "▾" } else { "▸" }
        } else {
            " "
        };
        let decoration = decorate(entry.item().id.as_ref());
        let tone = decoration.tone.unwrap_or_else(|| cx.theme().foreground);
        let label = decoration.label.map(|label| {
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().primary)
                .child(label)
        });
        ListItem::new(index).child(
            div()
                .h_flex()
                .gap_2()
                .pl(px(entry.depth() as f32 * ui::SPACE_MD))
                .child(div().w(px(ui::SPACE_MD)).child(marker))
                .child(div().text_color(tone).child(entry.item().label.clone()))
                .children(label),
        )
    })
    .context_menu(move |_index, entry, menu, window, cx| {
        let path = entry.item().id.to_string();
        build_menu(&path, menu, window, cx)
    })
}
