//! The numbers from DESIGN.md, in one place.
//!
//! Screens name a step instead of picking a padding or a font size per call
//! site, which is what keeps Windows and Linux looking like one product.

// The scale is the vocabulary every screen draws from. A step with no caller
// yet is not dead code; it is a step waiting for the screen that needs it.
#![allow(dead_code)]

use gpui_kit::*;

/// Spacing steps on the 4px grid.
pub const SPACE_XS: f32 = 4.;
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

/// Radius for in-page elements: buttons, inputs, list rows, bars.
pub const RADIUS: f32 = 4.;

/// Radius for top-level containers: windows, dialogs, flyouts.
pub const RADIUS_LG: f32 = 8.;

/// A line of text in a semantic color. Every empty state and every failure the
/// screens draw has this shape.
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

/// The last segment of a path, which is what a reader recognises a directory
/// by.
pub fn last_segment(path: &str) -> Option<String> {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .map(str::to_owned)
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
