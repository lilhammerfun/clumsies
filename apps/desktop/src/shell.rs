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
//! - a band across the top holding the page navigation, the open document as a
//!   tab, that document's view switch and its actions, and the window controls;
//! - the section's list and the work itself inside one floating card: rounded,
//!   bordered, a lighter colour than the page, and inset from the page's right
//!   and bottom edges;
//! - a panel to the right of the card saying what the client knows about what
//!   is open.
//!
//! A screen fills four slots: its list, its detail, its band, and — when it has
//! facts to offer — the right panel. Nothing else about a screen's layout is its
//! own business, which is what keeps the next six screens from each inventing a
//! window.
//!
//! Two deliberate differences from that reference: the window controls are this
//! platform's, at the right of the band rather than traffic lights at the left;
//! and the right panel holds facts, not tools, because the client has no tools
//! to offer there yet.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _, TitleBar};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::engine::Project;
use crate::ui::{self, Typography};

/// The rail of destinations: an icon, and nothing else.
pub const RAIL_WIDTH: f32 = 52.;
/// The open section's list column, inside the card.
pub const LIST_WIDTH: f32 = 240.;
/// The panel that says what is known about what is open.
pub const PANEL_WIDTH: f32 = 280.;
/// Below this the list and the work stack instead of sitting side by side,
/// which is the Windows rule for a window this narrow.
pub const STACK_WIDTH: f32 = 760.;
/// Below this the right panel folds away: it holds what the client knows, and
/// the work needs the width more than the facts do.
pub const PANEL_WIDTH_MIN: f32 = 1000.;
/// The gap between the floating card and the page it floats on.
pub const CARD_GAP: f32 = 8.;

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
            Section::Dashboard => {
                "Retrieval statistics for the Project, with the runs behind them."
            }
            Section::Inbox => "Reviews and mentions waiting on you.",
            Section::Memory => "",
            Section::Bundles => "Bundles of Memory resources, and where each one comes from.",
            Section::Reviews => "Open Reviews, newest first.",
            Section::Activity => "Retrieval runs and recall sessions.",
        }
    }

    pub fn detail_note(self) -> &'static str {
        match self {
            Section::Dashboard => {
                "Statistics and the retrieval runs behind them: DashboardPage in the macOS client."
            }
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

/// What the client knows about the engine it is talking to, as the right panel
/// names it.
pub struct EngineFacts {
    pub connected: bool,
    pub version: String,
    pub server: Option<String>,
    pub installation: Option<String>,
    pub schema: Option<i64>,
    /// Why it is not connected, when it is not.
    pub detail: Option<String>,
}

/// What the window supplies for its own chrome: which Project is open, the list
/// the picker offers, the engine behind it, what each destination has waiting,
/// and the width that decides what folds away.
pub struct Chrome<'a> {
    /// The Project the work belongs to.
    pub project: Option<&'a str>,
    /// The Projects the picker offers.
    pub projects: &'a [Project],
    /// The engine this client is talking to.
    pub engine: EngineFacts,
    /// What the account is called, for the foot of the rail.
    pub account: Option<&'a str>,
    /// The window's width, which decides what folds away.
    pub width: Pixels,
}

/// What a screen fills: its list column, its detail, its part of the top band,
/// and anything it has to say about what is open, which the right panel shows.
pub struct Slots {
    pub list: AnyElement,
    pub detail: AnyElement,
    /// The band's own content: what the screen adds beside the page navigation.
    pub band: AnyElement,
    pub inspector: Option<AnyElement>,
}

pub struct Shell {
    section: Section,
    /// The Project picker is chrome, not a screen, so the shell owns whether it
    /// is open.
    projects_open: bool,
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
            projects_open: false,
        }
    }

    pub fn section(&self) -> Section {
        self.section
    }

    pub fn close_projects(&mut self) {
        self.projects_open = false;
    }

    pub fn toggle_projects(&mut self) {
        self.projects_open = !self.projects_open;
    }

    pub fn set_section(&mut self, section: Section) {
        self.section = section;
        self.projects_open = false;
    }

    /// The chip that names the open Project. A screen puts it in its list
    /// column's header, which is where macOS keeps the same filter.
    pub fn project_picker(&self, chrome: &Chrome<'_>, cx: &mut Context<DesktopApp>) -> AnyElement {
        let project = chrome.project.unwrap_or("No Project").to_owned();
        div()
            .id("project-picker")
            .h_flex()
            .gap_1()
            .items_center()
            .px_2()
            .py_1()
            .rounded(px(ui::RADIUS))
            .text_style(&ui::CAPTION)
            .text_color(cx.theme().muted_foreground)
            .hover(|this| this.bg(cx.theme().list_hover))
            .child(ui::truncate(&project, 20))
            .child(Icon::new(IconName::ChevronDown).with_size(px(12.)))
            .on_click(cx.listener(|app, _event, _window, cx| app.toggle_projects(cx)))
            .into_any_element()
    }

    /// The window: the band across the top, then the rail and the card.
    pub fn render(
        &self,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
        chrome: Chrome<'_>,
        slots: Slots,
        actions: Option<AnyElement>,
    ) -> AnyElement {
        let narrow = chrome.width < px(STACK_WIDTH);
        let shows_panel = !narrow && chrome.width >= px(PANEL_WIDTH_MIN);
        let Slots {
            list,
            detail,
            band,
            inspector,
        } = slots;

        let list_column = div().v_flex().w(px(LIST_WIDTH)).h_full().child(list);
        let detail_column = div()
            .v_flex()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(detail);
        let inside = if narrow {
            div()
                .v_flex()
                .flex_1()
                .min_w(px(0.))
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
                .child(list_column)
                .child(divider(true, cx))
                .child(detail_column)
                .into_any_element()
        };

        // The work floats: a card of its own colour, inset from the page on
        // every side, which is what separates it from the chrome around it.
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
            .bg(cx.theme().background)
            .child(inside);

        let menu_open = self.projects_open;
        let overlay = menu_open.then(|| {
            div()
                .id("projects-overlay")
                .absolute()
                .inset_0()
                .occlude()
                .on_click(cx.listener(|app, _event, _window, cx| app.close_projects(cx)))
                .into_any_element()
        });
        let panel = menu_open.then(|| self.projects_panel(chrome.projects, cx));

        div()
            .v_flex()
            .relative()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.band(window, &chrome, band, actions, cx))
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.rail(&chrome, cx))
                    .child(card)
                    .children(shows_panel.then(|| self.inspector(&chrome, inspector, cx))),
            )
            .children(overlay)
            .children(panel)
            .into_any_element()
    }

    /// The band across the top: the window controls, the page navigation beside
    /// them, then whatever the open screen puts there and the window's actions.
    fn band(
        &self,
        window: &mut Window,
        chrome: &Chrome<'_>,
        band: AnyElement,
        actions: Option<AnyElement>,
        cx: &mut Context<DesktopApp>,
    ) -> AnyElement {
        let band = TitleBar::new().pl(px(0.)).child(
            div()
                .h_flex()
                .flex_1()
                .h_full()
                .items_center()
                .gap_2()
                .child(div().w(px(RAIL_WIDTH)).h_full())
                .child(nav_button("page-back", IconName::ArrowLeft, true, cx))
                .child(nav_button("page-forward", IconName::ArrowRight, true, cx))
                .child(div().w(px(ui::SPACE_SM)))
                .child(
                    div()
                        .h_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .items_center()
                        .child(band),
                )
                .children(actions),
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
                row.bg(cx.theme().sidebar_accent)
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
            .v_flex()
            .relative()
            .w(px(RAIL_WIDTH))
            .h_full()
            .py_2()
            .gap_1()
            .items_center()
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
            .child(
                div()
                    .id("account")
                    .h_flex()
                    .justify_center()
                    .items_center()
                    .size(px(36.))
                    .rounded(px(ui::RADIUS))
                    .hover(|this| this.bg(cx.theme().list_hover))
                    .child(
                        Icon::new(IconName::CircleUser)
                            .with_size(px(18.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .tooltip({
                        let label = chrome
                            .account
                            .map(str::to_owned)
                            .unwrap_or_else(|| "Signed in".to_owned());
                        move |window, cx| Tooltip::new(label.clone()).build(window, cx)
                    }),
            )
            .into_any_element()
    }

    /// The right panel: what the client knows about what is open, in small
    /// labelled groups. Everything in it is a fact the client already holds.
    fn inspector(
        &self,
        chrome: &Chrome<'_>,
        slot: Option<AnyElement>,
        cx: &mut Context<DesktopApp>,
    ) -> AnyElement {
        let engine = &chrome.engine;
        let mut facts = div().v_flex().gap_1().child(row(
            "state",
            if engine.connected {
                "connected".to_owned()
            } else {
                "unavailable".to_owned()
            },
            cx,
        ));
        if engine.connected {
            facts = facts
                .child(row("daemon", engine.version.clone(), cx))
                .children(
                    engine
                        .server
                        .as_ref()
                        .map(|server| row("server", server.clone(), cx)),
                )
                .children(
                    engine
                        .installation
                        .as_ref()
                        .map(|id| row("install", ui::shorten(id, 8), cx)),
                )
                .children(
                    engine
                        .schema
                        .map(|schema| row("schema", schema.to_string(), cx)),
                );
        }
        if let Some(detail) = &engine.detail {
            facts = facts.child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().danger)
                    .child(ui::truncate(detail, 60)),
            );
        }

        let mut panel = div()
            .v_flex()
            .w(px(PANEL_WIDTH))
            .h_full()
            .px_3()
            .pb_3()
            .gap_4();
        if let Some(slot) = slot {
            panel = panel.child(group("Open", slot, cx));
        }
        panel
            .child(group("Engine", facts.into_any_element(), cx))
            .into_any_element()
    }

    /// The Project list, under the chip that names the current Project. macOS
    /// keeps this list in the sidebar's own list; this client shows it where the
    /// Project is named.
    fn projects_panel(&self, projects: &[Project], cx: &mut Context<DesktopApp>) -> AnyElement {
        let items = projects.iter().enumerate().map(|(index, project)| {
            div()
                .id(("project-choice", index))
                .px_3()
                .py_2()
                .text_style(&ui::BODY)
                .child(project.name.clone())
                .hover(|this| this.bg(cx.theme().list_hover))
                .on_click(cx.listener(move |app, _event, _window, cx| {
                    app.choose_project(index, cx);
                }))
        });

        div()
            .id("projects-panel")
            .absolute()
            .top(px(ui::SPACE_2XL + ui::SPACE_MD))
            .left(px(RAIL_WIDTH + ui::SPACE_LG))
            .min_w(px(240.))
            .max_h(px(320.))
            .rounded(px(ui::RADIUS))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .v_flex()
            .children(items)
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
fn nav_button(
    id: &'static str,
    icon: IconName,
    disabled: bool,
    cx: &mut Context<DesktopApp>,
) -> AnyElement {
    let tone = if disabled {
        cx.theme().muted_foreground.opacity(0.5)
    } else {
        cx.theme().foreground
    };
    div()
        .id(id)
        .h_flex()
        .justify_center()
        .items_center()
        .size(px(24.))
        .rounded(px(ui::RADIUS))
        .child(Icon::new(icon).with_size(px(14.)).text_color(tone))
        .into_any_element()
}

/// Minimize, maximize and close, at the right of the band, which is where this
/// platform puts them.
fn window_controls(cx: &mut Context<DesktopApp>) -> AnyElement {
    div()
        .h_flex()
        .h_full()
        .items_center()
        .child(control(
            "window-minimize",
            IconName::WindowMinimize,
            false,
            |window| window.minimize_window(),
            cx,
        ))
        .child(control(
            "window-maximize",
            IconName::WindowMaximize,
            false,
            |window| window.zoom_window(),
            cx,
        ))
        .child(control(
            "window-close",
            IconName::WindowClose,
            true,
            |window| window.remove_window(),
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
    act: fn(&mut Window),
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
        .on_click(move |_, window, _| act(window))
        .child(Icon::new(icon).with_size(px(14.)))
        .into_any_element()
}

/// A labelled group in the right panel: a small muted heading and its rows.
fn group(label: &str, body: AnyElement, cx: &App) -> AnyElement {
    div()
        .v_flex()
        .gap_2()
        .child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(body)
        .into_any_element()
}

/// One fact: its name, and its value.
fn row(label: &str, value: String, cx: &App) -> AnyElement {
    div()
        .h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .w(px(64.))
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(div().text_style(&ui::CAPTION).child(value))
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
