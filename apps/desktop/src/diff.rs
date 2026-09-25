//! The diff view for a pending draft or a review.
//!
//! Line-level differences come from `similar`; this module owns the model and
//! the rendering. The model mirrors the macOS client's `UnifiedDiffLine` so the
//! two clients can describe the same change the same way.

use std::rc::Rc;

use gpui_kit::base::StyledExt;
use gpui_kit::*;
use similar::{ChangeTag, TextDiff};

/// What a patch row means. The macOS client also carries remote-only rows for
/// three-way conflicts; those arrive when the client talks to the engine.
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

const GUTTER: u32 = 0x8c959f;
const INSERTION_BG: u32 = 0xe6ffec;
const INSERTION_FG: u32 = 0x1a7f37;
const REMOVAL_BG: u32 = 0xffebe9;
const REMOVAL_FG: u32 = 0xcf222e;

/// A virtualized patch view: only the visible rows are built, so a long draft
/// or review stays cheap.
pub fn diff_view(rows: Vec<DiffRow>, mono_font: SharedString) -> impl IntoElement {
    let rows = Rc::new(rows);
    let count = rows.len();
    let render = {
        let rows = rows.clone();
        let mono_font = mono_font.clone();
        move |range: std::ops::Range<usize>, _window: &mut Window, _cx: &mut App| {
            range
                .map(|index| {
                    let row = &rows[index];
                    let (background, marker, marker_color) = match row.kind {
                        DiffKind::Context => (rgb(0xffffff), "", GUTTER),
                        DiffKind::Insertion => (rgb(INSERTION_BG), "+", INSERTION_FG),
                        DiffKind::Removal => (rgb(REMOVAL_BG), "-", REMOVAL_FG),
                    };
                    div()
                        .h_flex()
                        .w_full()
                        .bg(background)
                        .font_family(mono_font.clone())
                        .child(line_number(row.old_line))
                        .child(line_number(row.new_line))
                        .child(div().w(px(18.)).text_color(rgb(marker_color)).child(marker))
                        .child(div().child(row.text.clone()))
                })
                .collect::<Vec<_>>()
        }
    };
    uniform_list("draft-diff", count, render).size_full()
}

fn line_number(value: Option<usize>) -> Div {
    div()
        .w(px(44.))
        .pr_2()
        .text_color(rgb(GUTTER))
        .child(value.map(|value| value.to_string()).unwrap_or_default())
}
