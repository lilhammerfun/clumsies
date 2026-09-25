// The draft screen and the review screen are its callers and neither exists
// yet, so nothing constructs it today. That is a state, not an oversight: the
// model mirrors the macOS client and the rendering is proven, so the screens
// consume it rather than reinventing it.
#![allow(dead_code)]

//! The diff view for a pending draft or a review.
//!
//! Line-level differences come from `similar`; this module owns the model and
//! the rendering. The model mirrors the macOS client's `UnifiedDiffLine` so the
//! two clients describe a change the same way. Colors come from the theme, so
//! light and dark both work; the macOS client also carries remote-only rows for
//! three-way conflicts, which arrive when the engine does.

use std::rc::Rc;

use gpui_kit::base::StyledExt;
use gpui_kit::component::Theme;
use gpui_kit::*;
use similar::{ChangeTag, TextDiff};

use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Insertion,
    Removal,
}

pub struct DiffRow {
    pub kind: DiffKind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
}

/// Line-level differences between two versions of one document.
pub fn diff_rows(before: &str, after: &str) -> Vec<DiffRow> {
    TextDiff::from_lines(before, after)
        .iter_all_changes()
        .map(|change| {
            let (kind, old_line, new_line) = match change.tag() {
                ChangeTag::Equal => (DiffKind::Context, change.old_index(), change.new_index()),
                ChangeTag::Delete => (DiffKind::Removal, change.old_index(), None),
                ChangeTag::Insert => (DiffKind::Insertion, None, change.new_index()),
            };
            DiffRow {
                kind,
                old_line: old_line.map(|index| index + 1),
                new_line: new_line.map(|index| index + 1),
                text: change.value().trim_end_matches('\n').to_owned(),
            }
        })
        .collect()
}

/// The five colors a patch needs, resolved once from the theme.
pub struct DiffPalette {
    insertion: Hsla,
    insertion_bg: Hsla,
    removal: Hsla,
    removal_bg: Hsla,
    gutter: Hsla,
}

impl DiffPalette {
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            insertion: theme.success,
            // A full-strength success green behind a line is unreadable; the
            // tint carries the meaning and the marker carries the weight.
            insertion_bg: theme.success.opacity(0.12),
            removal: theme.danger,
            removal_bg: theme.danger.opacity(0.12),
            gutter: theme.muted_foreground,
        }
    }
}

/// A virtualized patch view: only the visible rows are built, so a long draft
/// or review stays cheap.
pub fn diff_view(
    rows: Vec<DiffRow>,
    mono_font: SharedString,
    palette: DiffPalette,
) -> impl IntoElement {
    let rows = Rc::new(rows);
    let count = rows.len();
    let render = move |range: std::ops::Range<usize>, _window: &mut Window, _cx: &mut App| {
        range
            .map(|index| {
                let row = &rows[index];
                let (background, marker, marker_color) = match row.kind {
                    DiffKind::Context => (palette.insertion_bg.opacity(0.), "", palette.gutter),
                    DiffKind::Insertion => (palette.insertion_bg, "+", palette.insertion),
                    DiffKind::Removal => (palette.removal_bg, "-", palette.removal),
                };
                div()
                    .h_flex()
                    .w_full()
                    .bg(background)
                    .font_family(mono_font.clone())
                    .child(line_number(row.old_line, palette.gutter, palette.removal))
                    .child(line_number(row.new_line, palette.gutter, palette.insertion))
                    .child(
                        div()
                            .w(px(ui::SPACE_LG))
                            .text_color(marker_color)
                            .child(marker),
                    )
                    .child(div().child(row.text.clone()))
            })
            .collect::<Vec<_>>()
    };
    uniform_list("draft-diff", count, render).size_full()
}

/// A changed line shows its own number in the color of the change, which is
/// how a reader tells "removed line 9" from "line 9 of the old file".
fn line_number(value: Option<usize>, empty: Hsla, changed: Hsla) -> Div {
    match value {
        Some(value) => div()
            .w(px(ui::SPACE_2XL + ui::SPACE_MD))
            .pr_2()
            .text_color(changed)
            .child(value.to_string()),
        None => div().w(px(ui::SPACE_2XL + ui::SPACE_MD)).pr_2().child(""),
    }
    .text_color(empty)
}
