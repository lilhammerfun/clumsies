//! Shared geometry for pane headers, groups, and their interactive controls.
use gpui_kit::base::StyledExt;
use gpui_kit::component::button::*;
use gpui_kit::*;

pub const HEIGHT: f32 = 44.;
pub const GROUP_HEIGHT: f32 = 40.;
pub const CONTROL_HEIGHT: f32 = 32.;
pub const RADIUS: f32 = 999.;

pub fn row() -> Div {
    div()
        .h_flex()
        .h(px(HEIGHT))
        .flex_shrink_0()
        .px(px(crate::ui::PANE_INSET))
        .gap_2()
        .items_center()
}

/// Layout only: background belongs to hover/selection, never to the wrapper.
pub fn group() -> Div {
    div()
        .h_flex()
        .h(px(GROUP_HEIGHT))
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded(px(RADIUS))
}

pub fn button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .ghost()
        .h(px(CONTROL_HEIGHT))
        .rounded(px(RADIUS))
}
