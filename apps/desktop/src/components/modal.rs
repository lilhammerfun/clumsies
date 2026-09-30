//! How this client opens a modal.
//!
//! Every dialog used to pick its own width, put its buttons inside the
//! scrolling content, and leave the library's dismissal defaults alone — which
//! meant four widths, footers that could scroll out of reach, and a form that
//! vanished when the pointer landed outside it. This is those decisions in one
//! place, so a dialog is opened the way every other dialog is.
//!
//! The three widths are steps rather than numbers: the small one is the
//! library's own default (448), the middle one holds a form of labelled fields,
//! and the large one holds a surface with a sidebar of its own. The rules:
//!
//! - **Escape always closes**, which the library does with `keyboard(true)`.
//! - **Clicking outside does not**, because a dialog here holds a form or a
//!   settings surface, and losing half-typed input to a stray click is worse
//!   than the extra Escape. macOS sheets behave the same way.
//! - **The actions live in the dialog's own footer**, never in the content, so
//!   they cannot scroll away from the reader who needs them.
//! - **The safe action comes first, the primary last**, which is where both
//!   platforms put them.

use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::*;
use gpui_kit::component::dialog::{Dialog, DialogFooter};
use gpui_kit::*;

use crate::ui::{self, Typography};

/// A dialog that asks one question: a title, one field or sentence, two buttons.
pub const NARROW: f32 = 448.;
/// A dialog that shows a form or a read-out: the common case.
pub const MEDIUM: f32 = 560.;
/// A dialog with a navigation column of its own, such as Settings.
pub const WIDE: f32 = 760.;

/// How tall the content of a surface grows before the library scrolls it.
pub const BODY: f32 = 520.;

/// Opens a dialog with this client's own conventions already applied.
///
/// The builder receives the dialog, so the caller adds its content and its
/// footer without repeating the rules.
pub fn open<F>(window: &mut Window, cx: &mut App, title: &str, width: f32, build: F)
where
    F: Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
{
    let title = title.to_owned();
    window.open_dialog(cx, move |dialog, window, cx| {
        let dialog = dialog
            .title(title.clone())
            .w(px(width))
            .keyboard(true)
            .overlay_closable(false);
        build(dialog, window, cx)
    });
}

/// The row a dialog ends with: what the reader can do about it.
///
/// `cancel` is the safe action and comes first; `primary` is what the dialog is
/// for and comes last, disabled while it cannot be done.
pub fn footer(cancel: Option<AnyElement>, primary: AnyElement) -> AnyElement {
    DialogFooter::new()
        .justify_end()
        .children(cancel)
        .child(primary)
        .into_any_element()
}

/// The button a reader presses to do the safe thing: leave without changing
/// anything. It closes the dialog because that is what it means, unless the
/// dialog is in the middle of something it would rather finish.
pub fn cancel(label: &str, enabled: bool) -> AnyElement {
    Button::new("modal-cancel")
        .label(label.to_owned())
        .disabled(!enabled)
        .on_click(|_event, window, cx| window.close_dialog(cx))
        .into_any_element()
}

/// The button a reader presses to do the thing the dialog is for.
pub fn primary(id: &'static str, label: &str, enabled: bool) -> Button {
    Button::new(id)
        .primary()
        .label(label.to_owned())
        .disabled(!enabled)
}

/// One line of a dialog that reports rather than asks: what it is, and what it
/// says. The label column is fixed so a stack of them lines up.
pub fn entry(label: &str, value: impl IntoElement, cx: &App) -> AnyElement {
    div()
        .h_flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .w(px(120.))
                .flex_shrink_0()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(div().flex_1().min_w(px(0.)).child(value))
        .into_any_element()
}

/// A heading inside a dialog, for a read-out of more than one group.
pub fn heading(text: &str, cx: &App) -> AnyElement {
    div()
        .pt(px(ui::SPACE_SM))
        .text_style(&ui::CAPTION)
        .text_color(cx.theme().muted_foreground)
        .child(text.to_owned())
        .into_any_element()
}
