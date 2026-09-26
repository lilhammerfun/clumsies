//! A tree of paths, and nothing else.
//!
//! Read from the macOS client's `PathTreeView`: one generic tree serves the
//! Memory navigator and the review's changed-file list, and each of them layers
//! its own decoration and its own commands on top. The generic part is the
//! projection (a flat list of paths becomes directories and files), the row, the
//! selection, and the commands every view has — open, edit, rename, delete;
//! everything a particular screen knows about a file is handed in from outside.
//!
//! Nothing here knows what Memory is.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

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

/// Which rows are selected, and the row a range grows from.
///
/// A tree of paths is selected by path: this component paints the set, and a
/// screen reads it to know what its commands act on. The macOS tree keeps the
/// same two pieces — the selected node ids, and the node a shift-click extends
/// from.
#[derive(Clone, Default)]
pub struct Selection {
    paths: BTreeSet<String>,
    anchor: Option<String>,
}

impl Selection {
    /// Whether more than one row is selected, which is the point at which a
    /// command acts on a batch rather than on a document.
    pub fn is_batch(&self) -> bool {
        self.paths.len() > 1
    }

    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// The selected rows, in path order rather than in the order they were
    /// clicked.
    pub fn paths(&self) -> impl Iterator<Item = &String> {
        self.paths.iter()
    }

    /// The row a range grows from, which is the row a plain click landed on.
    pub fn anchor(&self) -> Option<&str> {
        self.anchor.as_deref()
    }

    /// One row: what a plain click selects, and where a range starts.
    pub fn only(&mut self, path: &str) {
        self.paths.clear();
        self.paths.insert(path.to_owned());
        self.anchor = Some(path.to_owned());
    }

    /// Adds a row, or takes it away when it was already in. The row a range
    /// grows from stays where it was, unless it is the row that just left.
    pub fn toggle(&mut self, path: &str) {
        if self.paths.remove(path) {
            if self.anchor.as_deref() == Some(path) {
                self.anchor = self.paths.iter().next().cloned();
            }
            return;
        }
        self.paths.insert(path.to_owned());
        if self.anchor.is_none() {
            self.anchor = Some(path.to_owned());
        }
    }

    /// Everything between the row a range grows from and this one, in the order
    /// the tree shows them, which is what a shift-click selects.
    pub fn extend(&mut self, path: &str, order: &[String]) {
        let positions = self.anchor.as_deref().and_then(|anchor| {
            let from = order.iter().position(|row| row == anchor)?;
            let to = order.iter().position(|row| row == path)?;
            Some(if from <= to { (from, to) } else { (to, from) })
        });
        let Some((from, to)) = positions else {
            // Nothing to measure from: this row is the whole selection.
            self.only(path);
            return;
        };
        self.paths = order[from..=to].iter().cloned().collect();
    }

    pub fn clear(&mut self) {
        self.paths.clear();
        self.anchor = None;
    }

    /// What a click on one row means: a plain click selects it alone, a command
    /// click adds it to the set or takes it out, and a shift click takes
    /// everything between it and the row the range grows from. Every file list
    /// on every platform reads the three the same way.
    pub fn clicked(&mut self, path: &str, modifiers: Modifiers, order: &[String]) {
        if modifiers.shift {
            self.extend(path, order);
        } else if modifiers.control || modifiers.platform {
            self.toggle(path);
        } else {
            self.only(path);
        }
    }

    /// Forgets rows the tree no longer shows: a renamed file is not a selected
    /// file.
    pub fn retain(&mut self, order: &[String]) {
        self.paths.retain(|path| order.contains(path));
        if self
            .anchor
            .as_deref()
            .is_some_and(|anchor| !self.paths.contains(anchor))
        {
            self.anchor = self.paths.iter().next().cloned();
        }
    }
}

/// A click on a row: what it landed on, with what, and which button. A screen
/// decides what the click means — a plain left click opens a document, a
/// modified one extends the selection, and the right button asks for the row's
/// menu.
pub struct RowClick {
    pub path: String,
    pub button: MouseButton,
    pub modifiers: Modifiers,
}

impl RowClick {
    /// Whether the key that keeps a selection together was held.
    pub fn extending(&self) -> bool {
        self.modifiers.control || self.modifiers.platform || self.modifiers.shift
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

/// The tree, with a screen's decoration on every row, a screen's menu on every
/// row, and the selection the screen keeps. The path is handed to both the
/// decoration and the menu, because that is all a screen needs to decide what a
/// file looks like and what can be done with it.
pub fn path_tree(
    state: &Entity<TreeState>,
    selection: &Selection,
    decorate: impl Fn(&str) -> Decoration + 'static,
    on_click: impl Fn(RowClick, &mut Window, &mut App) + 'static,
    build_menu: impl Fn(&str, PopupMenu, &mut Window, &mut App) -> PopupMenu + 'static,
) -> impl IntoElement {
    let selected = Rc::new(selection.clone());
    let clicked = Rc::new(on_click);
    tree(state, move |index, entry, _lead, _window, cx| {
        let path = entry.item().id.to_string();
        let marker = if entry.is_folder() {
            if entry.is_expanded() { "▾" } else { "▸" }
        } else {
            " "
        };
        let decoration = decorate(&path);
        let tone = decoration.tone.unwrap_or_else(|| cx.theme().foreground);
        let label = decoration.label.map(|label| {
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().primary)
                .child(label)
        });
        let item = ListItem::new(index).child(
            div()
                .h_flex()
                .gap_2()
                .pl(px(entry.depth() as f32 * ui::SPACE_MD))
                .child(div().w(px(ui::SPACE_MD)).child(marker))
                .child(div().text_color(tone).child(entry.item().label.clone()))
                .children(label),
        );
        // A set of several rows is painted here: the component library
        // highlights one row, which is the row a plain click leaves behind.
        let item = if selected.is_batch() && selected.contains(&path) {
            item.bg(cx.theme().tokens.selection)
        } else {
            item
        };
        let press = clicked.clone();
        let pressed_path = path.clone();
        let menu_row = selected.clone();
        let menu_path = path.clone();
        let menu_click = clicked.clone();
        item.on_mouse_down(MouseButton::Left, move |event, window, cx| {
            // A modified click is about the set, not about the single row the
            // tree keeps as its own selection, so the row underneath keeps it.
            if event.modifiers.control || event.modifiers.platform || event.modifiers.shift {
                cx.stop_propagation();
            }
            (*press)(
                RowClick {
                    path: pressed_path.clone(),
                    button: MouseButton::Left,
                    modifiers: event.modifiers,
                },
                window,
                cx,
            );
        })
        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
            // A menu is about one row, or about the selection that row is part
            // of: a row outside the selection becomes the selection first,
            // which is what every file list does.
            if menu_row.contains(&menu_path) {
                return;
            }
            (*menu_click)(
                RowClick {
                    path: menu_path.clone(),
                    button: MouseButton::Right,
                    modifiers: event.modifiers,
                },
                window,
                cx,
            );
        })
    })
    .context_menu(move |_index, entry, menu, window, cx| {
        let path = entry.item().id.to_string();
        build_menu(&path, menu, window, cx)
    })
}

#[cfg(test)]
mod tests {
    // Only what the tests use: the component library exports a `test` macro of
    // its own, and a glob import would shadow the built-in attribute with it.
    use super::{Modifiers, Selection};

    /// The rows a tree of this Project's shape shows, in the order it shows
    /// them: directories first, then files.
    fn rows() -> Vec<String> {
        [
            "topics",
            "topics/rules.md",
            "guide.md",
            "notes.md",
            "reference.md",
        ]
        .iter()
        .map(|row| row.to_string())
        .collect()
    }

    fn command() -> Modifiers {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn a_plain_click_replaces_the_selection_and_a_command_click_adds_to_it() {
        let mut selection = Selection::default();
        selection.clicked("guide.md", Modifiers::default(), &rows());
        assert!(!selection.is_batch());
        assert!(selection.contains("guide.md"));
        assert_eq!(selection.anchor(), Some("guide.md"));

        selection.clicked("notes.md", command(), &rows());
        assert!(selection.is_batch());
        assert!(selection.contains("guide.md") && selection.contains("notes.md"));
        // A modified click does not move the row a range grows from.
        assert_eq!(selection.anchor(), Some("guide.md"));

        // Clicking a selected row with the key held takes it out again.
        selection.clicked("notes.md", command(), &rows());
        assert!(!selection.contains("notes.md"));
        assert!(!selection.is_batch());
    }

    #[test]
    fn a_shift_click_takes_every_row_between_the_anchor_and_it() {
        let mut selection = Selection::default();
        // The tree shows directories first, so the rows between these two are
        // the ones at 1..=3 of `rows()` — the range a reader sees, which is not
        // the order the set is kept in.
        selection.clicked("notes.md", Modifiers::default(), &rows());
        selection.clicked("topics/rules.md", shift(), &rows());
        assert_eq!(
            selection.paths().cloned().collect::<Vec<_>>(),
            ["guide.md", "notes.md", "topics/rules.md"]
        );
        // Backwards as well, from the row the range still grows from.
        selection.clicked("guide.md", shift(), &rows());
        assert_eq!(
            selection.paths().cloned().collect::<Vec<_>>(),
            ["guide.md", "notes.md"]
        );
    }

    #[test]
    fn a_row_the_tree_no_longer_shows_is_not_selected() {
        let mut selection = Selection::default();
        selection.clicked("notes.md", Modifiers::default(), &rows());
        selection.clicked("topics", command(), &rows());
        selection.retain(&["guide.md".to_owned(), "topics".to_owned()]);
        assert!(!selection.contains("notes.md"));
        assert!(selection.contains("topics"));
        // The row a range grows from was the one that left, so another member
        // of the set takes its place.
        assert_eq!(selection.anchor(), Some("topics"));
    }
}
