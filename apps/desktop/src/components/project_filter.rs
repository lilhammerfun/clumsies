//! Project scope menu, matching macOS's ToolbarFilterMenu composition.
use super::header;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::*;
use std::rc::Rc;

pub fn project_filter(
    projects: Vec<String>,
    selected: Option<usize>,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let title = selected
        .and_then(|index| projects.get(index))
        .cloned()
        .unwrap_or_else(|| "Select Project".into());
    let on_select = Rc::new(on_select);
    let button = header::button("project-filter")
        .icon(Icon::default().path("icons/list-filter.svg"))
        .accessibility_label(format!("Project Filter: {title}"))
        .tooltip(title.clone())
        .child(div().max_w(px(150.)).truncate().child(title))
        .child(Icon::new(IconName::ChevronDown).with_size(px(12.)))
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
            if projects.is_empty() {
                return menu.item(PopupMenuItem::new("No Projects").disabled(true));
            }
            for (index, name) in projects.iter().enumerate() {
                let on_select = on_select.clone();
                menu = menu.item(
                    PopupMenuItem::new(name.clone())
                        .checked(selected == Some(index))
                        .on_click(move |_, window, cx| {
                            if selected != Some(index) {
                                on_select(index, window, cx);
                            }
                        }),
                );
            }
            menu
        });
    header::group().child(button).into_any_element()
}
