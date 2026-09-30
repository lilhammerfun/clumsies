//! Project scope menu, matching macOS's ToolbarFilterMenu composition.
use super::header;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::*;
use std::rc::Rc;

/// The Project filter: every memory space this account can reach, the
/// Organization's own Memory, and the one action that makes a new space.
///
/// `selected` is `None` for the Organization's Memory, which is the entry the
/// window already knew how to show — the menu simply never offered it.
pub fn project_filter(
    projects: Vec<String>,
    selected: Option<usize>,
    on_select: impl Fn(Option<usize>, &mut Window, &mut App) + 'static,
    on_create: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let title = selected
        .and_then(|index| projects.get(index))
        .cloned()
        .unwrap_or_else(|| "Organization Memory".into());
    let on_select = Rc::new(on_select);
    let on_create = Rc::new(on_create);
    let button = header::button("project-filter")
        .icon(Icon::default().path("icons/list-filter.svg"))
        .accessibility_label(format!("Project Filter: {title}"))
        .tooltip(title.clone())
        .child(div().max_w(px(150.)).truncate().child(title))
        .child(Icon::new(IconName::ChevronDown).with_size(px(12.)))
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
            if projects.is_empty() {
                menu = menu.item(PopupMenuItem::new("No memory spaces yet").disabled(true));
            }
            for (index, name) in projects.iter().enumerate() {
                let on_select = on_select.clone();
                menu = menu.item(
                    PopupMenuItem::new(name.clone())
                        .checked(selected == Some(index))
                        .on_click(move |_, window, cx| {
                            if selected != Some(index) {
                                on_select(Some(index), window, cx);
                            }
                        }),
                );
            }
            // The Organization's Memory is the whole of what the account can
            // read across Projects; adding it to a Project is the Memory
            // screen's own action, which is why this entry only shows it.
            let organization = on_select.clone();
            menu = menu
                .separator()
                .item(
                    PopupMenuItem::new("Organization Memory")
                        .checked(selected.is_none())
                        .on_click(move |_, window, cx| {
                            organization(None, window, cx);
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("New Project…").on_click({
                    let on_create = on_create.clone();
                    move |_, window, cx| on_create(window, cx)
                }));
            menu
        });
    header::group().child(button).into_any_element()
}
