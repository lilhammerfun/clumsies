//! The window shell: what every screen shares.
//!
//! Read from the macOS client's WorkspaceView — a NavigationSplitView whose
//! sidebar is GlobalSidebar, whose content column is the open section's
//! navigator, and whose detail is the work — and laid out the way the reference
//! window places its regions:
//!
//! - a rail of destinations down the left, icons only, with a badge where a
//!   destination has something waiting and the help and account affordances at
//!   the foot;
//! - a band across the top belonging to the window alone: the page navigation
//!   and the window controls, and nothing a screen owns;
//! - the section's list and the work itself inside one floating card: rounded,
//!   bordered, a content canvas distinct from the page, and inset from the page's right
//!   and bottom edges. Each of those two panes carries a header row of its own
//!   for the commands and facts that act on that pane — the list's header, and
//!   the document's header — which is where a screen puts what it offers, and
//!   why the band stays empty of them.
//!
//! A screen fills two slots: its list and its detail. Nothing else about a
//! screen's layout is its own business, which is what keeps the next six screens
//! from each inventing a window.
//!
//! Two deliberate differences from that reference: the window controls are this
//! platform's, at the right of the band rather than traffic lights at the left;
//! and the commands macOS keeps in the window toolbar are drawn in each pane's
//! header instead, because a command belongs beside the region it acts on.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::*;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _, TitleBar};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::ui::{self, Typography};

/// The rail of destinations: an icon, and nothing else.
pub const RAIL_WIDTH: f32 = 52.;
/// The open section's list column, inside the card.
pub const LIST_WIDTH: f32 = 240.;
/// Below this the list and the work stack instead of sitting side by side,
/// which is the Windows rule for a window this narrow.
pub const STACK_WIDTH: f32 = 760.;
/// The gap between the floating card and the page it floats on.
pub const CARD_GAP: f32 = 8.;

/// How wide a screen's own area is in a window of this width, which is what a
/// screen lays its own columns out by. The shell owns the rail, the list column,
/// the divider between them and the card's own inset, so a screen asks for the
/// width it will be given rather than subtracting them itself.
///
/// `listed` says whether the open section fills a list column. A section that
/// fills none — macOS's Dashboard has no navigator — has the whole card, and a
/// narrow window stacks the list above the work, so the work has the width
/// either way there.
pub fn content_width(window_width: Pixels, listed: bool) -> Pixels {
    let chrome = px(RAIL_WIDTH + 1. + 2. * CARD_GAP);
    match (listed, window_width < px(STACK_WIDTH)) {
        (false, _) => window_width - chrome,
        (true, true) => window_width,
        (true, false) => window_width - chrome - px(LIST_WIDTH),
    }
}

/// The six destinations of the macOS client, in its order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Dashboard,
    Inbox,
    Memory,
    Bundles,
    Reviews,
    Activity,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Dashboard,
        Section::Inbox,
        Section::Memory,
        Section::Bundles,
        Section::Reviews,
        Section::Activity,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Dashboard => "Dashboard",
            Section::Inbox => "Inbox",
            Section::Memory => "Memory",
            Section::Bundles => "Bundles",
            Section::Reviews => "Reviews",
            Section::Activity => "Activity",
        }
    }

    /// The macOS client's own symbol for this destination, in this client's icon
    /// set: WorkspaceSection.symbol in MemoryModels.swift. Memory is the brain
    /// there and the brain here, which is why these are drawn by path rather
    /// than from the component library's curated names.
    fn symbol(self) -> &'static str {
        match self {
            // chart.bar.xaxis
            Section::Dashboard => "icons/chart-bar.svg",
            // tray
            Section::Inbox => "icons/inbox.svg",
            // brain
            Section::Memory => "icons/brain.svg",
            // shippingbox
            Section::Bundles => "icons/package.svg",
            // checkmark.bubble
            Section::Reviews => "icons/message-square-check.svg",
            // bubble.left.and.bubble.right
            Section::Activity => "icons/messages-square.svg",
        }
    }

    fn icon(self) -> Icon {
        Icon::default().path(self.symbol())
    }

    /// What this section will list, in the terms of the macOS screen it comes
    /// from. A section that is not built yet says so, rather than drawing an
    /// empty column with no explanation.
    pub fn list_note(self) -> &'static str {
        match self {
            Section::Dashboard => "",
            Section::Inbox => "Reviews and mentions waiting on you.",
            Section::Memory => "",
            Section::Bundles => "Bundles of Memory resources, and where each one comes from.",
            Section::Reviews => "Open Reviews, newest first.",
            Section::Activity => "Retrieval runs and recall sessions.",
        }
    }

    pub fn detail_note(self) -> &'static str {
        match self {
            Section::Dashboard => "",
            Section::Inbox => "Reviews and mentions waiting on you: InboxView in the macOS client.",
            Section::Memory => "",
            Section::Bundles => "Bundles of Memory resources: BundlesView in the macOS client.",
            Section::Reviews => {
                "The proposals, their diff, and the decision: ReviewsView in the macOS client."
            }
            Section::Activity => {
                "Retrieval runs and recall sessions: ActivityView in the macOS client."
            }
        }
    }
}

/// What the rail's foot says about the engine it is talking to: whether it is
/// there, and which daemon it is.
pub struct EngineFacts {
    pub connected: bool,
    pub version: String,
}

/// The account the rail's foot stands for: the identity a menu names it by,
/// and the Organization it belongs to.
#[derive(Clone, Copy)]
pub struct AccountFacts<'a> {
    /// macOS's `identityLabel`: the name the reader answers to.
    pub identity: &'a str,
    /// macOS's `loginLabel`: what the account signs in with.
    pub sign_in_as: &'a str,
    pub organization: &'a str,
}

/// Shared window navigation, engine state and responsive layout.
pub struct Chrome<'a> {
    /// The engine this client is talking to.
    pub engine: EngineFacts,
    /// Whose account this is, for the foot of the rail and the menu behind it.
    /// Absent while the engine holds no Server session.
    pub account: Option<AccountFacts<'a>>,
    /// Whether the open screen has somewhere to go back to, and forward to.
    /// The band's arrows are drawn from these two.
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// Where the rail takes the keyboard, and whether it has it: the rail is a
    /// region F6 walks, and the section it is on says so with a ring.
    pub rail_focus: &'a FocusHandle,
    pub rail_focused: bool,
    /// The window's width, which decides what folds away.
    pub width: Pixels,
}

/// What a screen fills: its list column and its detail. Everything a screen
/// has to show or offer belongs to one of the two panes, each of which carries
/// its own header; the band above them belongs to the window.
///
/// A screen that has no navigator — as macOS's Dashboard has none — fills no
/// list, and the detail takes the whole card.
pub struct Slots {
    pub list: Option<AnyElement>,
    pub detail: AnyElement,
}

pub struct Shell {
    section: Section,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    pub fn new() -> Self {
        Self {
            section: Section::Memory,
        }
    }

    pub fn section(&self) -> Section {
        self.section
    }

    pub fn set_section(&mut self, section: Section) {
        self.section = section;
    }

    /// The window: the band across the top, then the rail and the card.
    pub fn render(
        &self,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
        chrome: Chrome<'_>,
        slots: Slots,
    ) -> AnyElement {
        let narrow = chrome.width < px(STACK_WIDTH);
        let Slots { list, detail } = slots;

        let detail_column = div()
            .v_flex()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(detail);
        // A section without a navigator fills the card itself: macOS's
        // Dashboard is a sidebar beside one page, and that page holds the
        // metric cards as well as the panels.
        //
        // Every wrapper in this chain carries `min_h(0)`: a flex item's
        // automatic minimum height is its content's, so without it a page
        // taller than the window stretches this row instead of scrolling
        // inside it, and the bottom of the page is simply cut off.
        let inside = match list {
            None => div()
                .h_flex()
                .items_stretch()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .child(detail_column)
                .into_any_element(),
            Some(list) => {
                let list_column = div().v_flex().w(px(LIST_WIDTH)).h_full().child(list);
                if narrow {
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .child(div().v_flex().h(px(180.)).child(list_column))
                        .child(divider(false, cx))
                        .child(detail_column)
                        .into_any_element()
                } else {
                    div()
                        .h_flex()
                        .items_stretch()
                        .flex_1()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .child(list_column)
                        .child(divider(true, cx))
                        .child(detail_column)
                        .into_any_element()
                }
            }
        };

        // The content uses the base canvas, framed by the contrasting page chrome.
        let card = div()
            .v_flex()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .mx(px(CARD_GAP))
            .mb(px(CARD_GAP))
            .rounded(px(ui::RADIUS_LG))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .bg(ui::content_background(cx))
            .child(inside);

        div()
            .v_flex()
            .relative()
            .size_full()
            .bg(ui::page_background(cx))
            .child(self.band(window, &chrome, cx))
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.rail(&chrome, cx))
                    .child(card),
            )
            .into_any_element()
    }

    /// The band across the top belongs to the window, not to a screen: the page
    /// navigation on the left, the window controls on the right, and nothing
    /// else. Commands a screen offers are drawn in that screen's own panes, next
    /// to what they act on.
    fn band(
        &self,
        window: &mut Window,
        chrome: &Chrome<'_>,
        cx: &mut Context<DesktopApp>,
    ) -> AnyElement {
        // The band is the page: no surface of its own, no rule under it, so the
        // window reads as one surface with a card floating on it.
        let band = TitleBar::new()
            .pl(px(0.))
            .bg(ui::page_background(cx))
            .border_color(ui::page_background(cx))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(RAIL_WIDTH)).h_full())
                    .child(nav_button(
                        "page-back",
                        IconName::ArrowLeft,
                        chrome.can_go_back,
                        |app, _event, window, cx| app.go_back(window, cx),
                        cx,
                    ))
                    .child(nav_button(
                        "page-forward",
                        IconName::ArrowRight,
                        chrome.can_go_forward,
                        |app, _event, window, cx| app.go_forward(window, cx),
                        cx,
                    ))
                    .child(div().flex_1().min_w(px(0.))),
            );
        let band = if draws_own_controls(window) {
            band.child(window_controls(cx))
        } else {
            band
        };
        band.into_any_element()
    }

    /// The destinations, each an icon in a square, with a badge where a
    /// destination has something waiting.
    fn rail(&self, chrome: &Chrome<'_>, cx: &mut Context<DesktopApp>) -> AnyElement {
        let rows = Section::ALL.into_iter().map(|section| {
            let selected = section == self.section;
            let tone = if selected {
                cx.theme().sidebar_accent_foreground
            } else {
                cx.theme().muted_foreground
            };
            let row = div()
                .id(("section", section as usize))
                .h_flex()
                .justify_center()
                .items_center()
                .size(px(36.))
                .rounded(px(ui::RADIUS))
                .child(section.icon().with_size(px(18.)).text_color(tone))
                .tooltip({
                    let title = section.title();
                    move |window, cx| Tooltip::new(title).build(window, cx)
                });
            let row = if selected {
                let row = row.bg(cx.theme().sidebar_accent);
                if chrome.rail_focused {
                    row.border_1().border_color(cx.theme().ring)
                } else {
                    row
                }
            } else {
                row.hover(|this| this.bg(cx.theme().list_hover))
            };
            let row = match badge(section) {
                Some(count) => row.child(
                    div()
                        .absolute()
                        .top(px(ui::SPACE_XS))
                        .right(px(ui::SPACE_XS))
                        .px_1()
                        .rounded_full()
                        .bg(cx.theme().primary)
                        .text_style(&ui::CAPTION)
                        .text_color(cx.theme().primary_foreground)
                        .child(count),
                ),
                None => row,
            };
            row.on_click(cx.listener(move |app, _event, _window, cx| {
                app.select_section(section, cx);
            }))
        });

        div()
            .id("rail")
            .v_flex()
            .relative()
            .w(px(RAIL_WIDTH))
            .h_full()
            .py_2()
            .gap_1()
            .items_center()
            .track_focus(chrome.rail_focus)
            .tab_stop(true)
            .children(rows)
            .child(div().flex_1())
            .child(self.rail_foot(chrome, cx))
            .into_any_element()
    }

    /// The foot of the rail: what a reader reaches for when the work is not
    /// what they need — how the engine is doing, and whose account this is.
    fn rail_foot(&self, chrome: &Chrome<'_>, cx: &mut Context<DesktopApp>) -> AnyElement {
        div()
            .v_flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("engine-state")
                    .h_flex()
                    .justify_center()
                    .items_center()
                    .size(px(36.))
                    .rounded(px(ui::RADIUS))
                    .hover(|this| this.bg(cx.theme().list_hover))
                    .child(engine_dot(chrome, cx))
                    .tooltip({
                        let label = if chrome.engine.connected {
                            format!("daemon {}", chrome.engine.version)
                        } else {
                            "engine unavailable".to_owned()
                        };
                        move |window, cx| Tooltip::new(label.clone()).build(window, cx)
                    })
                    .on_click(cx.listener(|app, _event, _window, cx| app.recheck_engine(cx))),
            )
            .child(self.account_button(chrome, cx))
            .into_any_element()
    }

    /// Whose account this is, and the two things a reader does about it: open
    /// Settings, or sign out. macOS keeps the same menu at the foot of its
    /// sidebar — the identity, then Settings, then Sign Out under a rule — and
    /// this is the rail's foot doing that job.
    fn account_button(&self, chrome: &Chrome<'_>, cx: &mut Context<DesktopApp>) -> AnyElement {
        // The menu is built once and outlives the frame, so what it names is
        // owned rather than borrowed from the chrome this frame was drawn with.
        let identity = chrome.account.map(|account| account.identity.to_owned());
        let sign_in_as = chrome.account.map(|account| account.sign_in_as.to_owned());
        let organization = chrome
            .account
            .map(|account| account.organization.to_owned());
        let this = cx.entity();
        let settings = this.clone();
        let sign_out = this.clone();
        let label = identity
            .map(|identity| match &organization {
                Some(organization) => format!("{identity} · {organization}"),
                None => identity,
            })
            .unwrap_or_else(|| "Not signed in".to_owned());
        Button::new("account")
            .ghost()
            .h(px(36.))
            .w(px(36.))
            .icon(
                Icon::new(IconName::CircleUser)
                    .with_size(px(18.))
                    .text_color(cx.theme().muted_foreground),
            )
            .tooltip(label)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _window, _cx| {
                let mut menu = menu;
                // The identity is named, not offered: it is what the two
                // commands below act on.
                if let Some(sign_in_as) = sign_in_as.clone() {
                    menu = menu.item(PopupMenuItem::new(sign_in_as).disabled(true));
                }
                menu.separator()
                    .item(PopupMenuItem::new("Settings…").on_click({
                        let settings = settings.clone();
                        move |_event, window, cx| {
                            settings.update(cx, |app, cx| app.open_settings(window, cx));
                        }
                    }))
                    .item(PopupMenuItem::new("Sign Out").on_click({
                        let sign_out = sign_out.clone();
                        move |_event, window, cx| {
                            sign_out.update(cx, |app, cx| app.sign_out(window, cx));
                        }
                    }))
            })
            .into_any_element()
    }
}

/// What a destination has waiting for the reader, when anything does. Only the
/// Inbox carries a count in macOS — the unread inbox — and the client has no
/// unread count to show until that screen exists, so this answers nothing
/// today and is the one place it will answer from.
fn badge(_section: Section) -> Option<String> {
    None
}

/// Whether this window has to draw its own window controls.
///
/// The component library skips them under server-side decorations, on the
/// grounds that the window manager draws a title bar with its own. Some
/// compositors answer the decoration request with "server" and then draw a
/// border and nothing else — Hyprland does exactly that — so a client that
/// leaves the buttons to the platform ends up with none at all.
fn draws_own_controls(window: &Window) -> bool {
    cfg!(target_os = "linux") && matches!(window.window_decorations(), Decorations::Server)
}

/// One page navigation button. The history itself arrives with the tab strip:
/// the buttons are here so the band reads the way the reference reads, and they
/// are disabled rather than pretending.
/// One of the band's page arrows. An arrow with nowhere to go is drawn faint
/// and takes no clicks, which is how the platform draws a disabled control.
fn nav_button(
    id: &'static str,
    icon: IconName,
    enabled: bool,
    action: impl Fn(&mut DesktopApp, &ClickEvent, &mut Window, &mut Context<DesktopApp>) + 'static,
    cx: &mut Context<DesktopApp>,
) -> AnyElement {
    let tone = if enabled {
        cx.theme().foreground
    } else {
        cx.theme().muted_foreground.opacity(0.5)
    };
    let button = div()
        .id(id)
        .h_flex()
        .justify_center()
        .items_center()
        .size(px(24.))
        .rounded(px(ui::RADIUS))
        .child(Icon::new(icon).with_size(px(14.)).text_color(tone));
    let button = if enabled {
        button
            .hover(|style| style.bg(cx.theme().secondary_hover))
            .cursor_pointer()
            .on_click(cx.listener(action))
    } else {
        button
    };
    button.into_any_element()
}

/// Minimize, maximize and close, at the right of the band, which is where this
/// platform puts them.
fn window_controls(cx: &mut Context<DesktopApp>) -> AnyElement {
    let app = cx.entity();
    div()
        .h_flex()
        .h_full()
        .items_center()
        .child(control(
            "window-minimize",
            IconName::WindowMinimize,
            false,
            |window, _cx| window.minimize_window(),
            cx,
        ))
        .child(control(
            "window-maximize",
            IconName::WindowMaximize,
            false,
            |window, _cx| window.zoom_window(),
            cx,
        ))
        .child(control(
            "window-close",
            IconName::WindowClose,
            true,
            // What the reader typed in the last pause belongs to the engine
            // before the window goes, and this is the last moment it can.
            move |window, cx| {
                if app.update(cx, |app, cx| app.flush_pending_saves(cx)) {
                    window.remove_window();
                }
            },
            cx,
        ))
        .into_any_element()
}

/// One control of the three: a square that takes the hover a reader expects,
/// with the close button carrying the danger colour.
fn control(
    id: &'static str,
    icon: IconName,
    danger: bool,
    act: impl Fn(&mut Window, &mut App) + 'static,
    cx: &mut Context<DesktopApp>,
) -> AnyElement {
    div()
        .id(id)
        .h_flex()
        .justify_center()
        .items_center()
        .w(px(46.))
        .h_full()
        .text_color(cx.theme().foreground)
        .hover(move |this| {
            if danger {
                this.bg(cx.theme().danger)
                    .text_color(cx.theme().danger_foreground)
            } else {
                this.bg(cx.theme().secondary_hover)
            }
        })
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| act(window, cx))
        .child(Icon::new(icon).with_size(px(14.)))
        .into_any_element()
}

/// A dot in the colour of the engine's state, so a glance says whether the
/// client is talking to anything.
fn engine_dot(chrome: &Chrome<'_>, cx: &App) -> AnyElement {
    let color = if chrome.engine.connected {
        cx.theme().success
    } else {
        cx.theme().danger
    };
    div()
        .size(px(10.))
        .rounded_full()
        .bg(color)
        .into_any_element()
}

/// A one-pixel rule between regions. The layout has no border widths for single
/// edges, so a rule is an element like any other.
fn divider(vertical: bool, cx: &App) -> AnyElement {
    let rule = div().bg(cx.theme().border);
    if vertical {
        rule.w(px(1.)).h_full().into_any_element()
    } else {
        rule.h(px(1.)).into_any_element()
    }
}
