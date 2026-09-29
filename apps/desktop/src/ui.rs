//! The numbers from DESIGN.md, in one place.
//!
//! Screens name a step instead of picking a padding or a font size per call
//! site, which is what keeps Windows and Linux looking like one product.

// The scale is the vocabulary every screen draws from. A step with no caller
// yet is not dead code; it is a step waiting for the screen that needs it.
#![allow(dead_code)]

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

/// Spacing steps on the 4px grid.
pub const SPACE_XS: f32 = 4.;
/// Shared outer horizontal inset for pane toolbars and navigator lists.
pub const PANE_INSET: f32 = SPACE_XS;
pub const SPACE_SM: f32 = 8.;
pub const SPACE_MD: f32 = 12.;
pub const SPACE_LG: f32 = 16.;
pub const SPACE_XL: f32 = 24.;
pub const SPACE_2XL: f32 = 32.;

/// One entry of the Windows type ramp.
pub struct TextStyle {
    pub size: f32,
    pub line_height: f32,
}

/// Small, 12/16. Windows states this as the floor for legible text, and every
/// screen of ours carries Chinese, so nothing goes below it.
pub const CAPTION: TextStyle = TextStyle {
    size: 12.,
    line_height: 16.,
};

/// Text, 14/20. The default for UI text.
pub const BODY: TextStyle = TextStyle {
    size: 14.,
    line_height: 20.,
};

/// Text, 18/24.
pub const BODY_LARGE: TextStyle = TextStyle {
    size: 18.,
    line_height: 24.,
};

/// Display semibold, 20/28.
pub const SUBTITLE: TextStyle = TextStyle {
    size: 20.,
    line_height: 28.,
};

/// Display semibold, 28/36. The largest step a screen has asked for so far: a
/// Dashboard card's figure.
pub const TITLE: TextStyle = TextStyle {
    size: 28.,
    line_height: 36.,
};

/// Radius for in-page elements: buttons, inputs, list rows, bars.
pub const RADIUS: f32 = 4.;

/// Radius for top-level containers: windows, dialogs, flyouts.
pub const RADIUS_LG: f32 = 8.;

/// The canvas and chrome use GPUI's existing semantic colors, without mixing.
pub fn content_background(cx: &App) -> Hsla {
    cx.theme().background
}

pub fn page_background(cx: &App) -> Hsla {
    cx.theme().title_bar
}

pub fn selected_background(cx: &App) -> Hsla {
    cx.theme().accent
}

/// One step of the brand's ramp, from the same hex the site and the docs use.
fn hsla_of(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// The product's own accent, which the theme does not carry.
///
/// The library's default maps `primary` and `link` to the foreground colour, so
/// in dark mode every primary action and every link would be white — the
/// sign-in page's two links, a dialog's confirm button, the review actions. The
/// macOS client draws those in `Color.accentColor`, the system's accent; this
/// client has an identity of its own instead, and it is the brand's ramp, the
/// one the site and the documentation already use: `#e657a0` light and
/// `#f178b8` dark for the actions, a shade deeper for a link at rest, the
/// lighter step for hover, the deepest for the pressed state, and the brand's
/// own wash behind selected text.
///
/// It is written into the theme rather than applied per control, because it is
/// one decision: a second button with its own colour is a second accent.
pub fn apply_brand(cx: &mut App) {
    use gpui_kit::component::Theme;
    let (action, action_text, link, hover, active, selection) = if cx.theme().mode.is_dark() {
        (
            hsla_of(0xf178b8),
            hsla_of(0x2a1220),
            hsla_of(0xf178b8),
            hsla_of(0xff9ccb),
            hsla_of(0xffd4ea),
            hsla_of(0xf178b8).alpha(0.18),
        )
    } else {
        (
            hsla_of(0xe657a0),
            hsla_of(0xffffff),
            hsla_of(0xc93b86),
            hsla_of(0xe657a0),
            hsla_of(0xa82f6f),
            hsla_of(0xe657a0).alpha(0.12),
        )
    };
    let mut tokens = cx.theme().semantic_tokens();
    tokens.colors.primary = action;
    tokens.colors.primary_foreground = action_text;
    // The focus ring is the accent too, which is what makes a focused control
    // read as the same product as the button beside it.
    tokens.colors.ring = link;
    tokens.colors.selection = selection;
    let theme = Theme::global_mut(cx);
    theme.apply_semantic_tokens(&tokens);
    // Lists use the same neutral selection surface as document tabs.
    theme.list.active_highlight = false;
    theme.link = link;
    theme.link_hover = hover;
    theme.link_active = active;
    gpui_kit::base::Theme::global_mut(cx).tokens = tokens;
}

/// Keep every GPUI popup on the same surface role as the surrounding chrome
/// in dark mode. Light mode retains the library's white popup surface.
/// This is a theme mapping, not a per-menu style or a new color palette.
pub fn sync_popup_surface(cx: &mut App) {
    use gpui_kit::component::Theme;
    let mut tokens = cx.theme().semantic_tokens();
    if cx.theme().mode.is_dark() {
        tokens.colors.surface = cx.theme().title_bar;
        tokens.colors.surface_foreground = cx.theme().foreground;
    }
    Theme::global_mut(cx).apply_semantic_tokens(&tokens);
    gpui_kit::base::Theme::global_mut(cx).tokens = tokens;
}

/// A line of text in a semantic color. Every empty state and every failure the
/// screens draw has this shape.
/// A hairline rule across a pane, under a header row. Screens use this rather
/// than a border on one edge, which this framework does not offer.
pub fn rule(cx: &App) -> AnyElement {
    div()
        .h(px(1.))
        .w_full()
        .flex_shrink_0()
        .bg(cx.theme().border)
        .into_any_element()
}

pub fn message(text: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .text_style(&BODY)
        .text_color(color)
        .child(text.into())
        .into_any_element()
}

/// A long value cut to what a bar can hold.
pub fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let kept: String = value.chars().take(limit.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// The tail of an identity, which is what tells two of them apart.
pub fn shorten(value: &str, keep: usize) -> String {
    let characters = value.chars().count();
    if characters <= keep {
        return value.to_owned();
    }
    let tail: String = value.chars().skip(characters - keep).collect();
    format!("…{tail}")
}

/// Applies a ramp entry to any styled element.
pub trait Typography: Styled + Sized {
    fn text_style(self, style: &TextStyle) -> Self {
        self.text_size(px(style.size))
            .line_height(px(style.line_height))
    }
}

impl<T: Styled> Typography for T {}
