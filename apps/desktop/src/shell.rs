//! The window shell: what every screen shares.
//!
//! Read from the macOS client's WorkspaceView — a NavigationSplitView whose
//! sidebar is GlobalSidebar, whose content column is the open section's
//! navigator, and whose detail is the work — and laid out the way a reference
//! window of this kind places its regions: a band across the top for the window
//! controls, a rail of destinations down the left, the open section's list
//! beside it, the work in the middle under its own header and over its own
//! action bar, and a panel on the right for what is known about the thing on
//! screen.
//!
//! A screen fills three slots: its list, its detail, and — when it has
//! something to say about what is open — the right panel. Nothing else about a
//! screen's layout is its own business, which is what keeps the next six
//! screens from each inventing a window.
//!
//! Three deliberate differences from that reference:
//!
//! - **The window controls are the platform's**, at the right of the top band:
//!   minimize, maximize and close. The reference is a macOS window, where the
//!   platform puts them at the left; a reader's hands know this platform's.
//! - **The band and the rail are one surface.** The corner above the rail
//!   carries the rail's colour, so the navigation reaches the window's top edge.
//! - **The right panel holds facts, not tools yet.** It names the document and
//!   its draft, and the engine the client is talking to, all of which the
//!   client already knows; a panel of empty promises would be worse than none.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _, TitleBar};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::engine::Project;
use crate::ui::{self, Typography};

/// The rail of destinations: an icon and the name beside it.
pub const RAIL_WIDTH: f32 = 168.;
/// The open section's list column.
pub const LIST_WIDTH: f32 = 240.;
/// The panel that says what is known about what is open.
pub const PANEL_WIDTH: f32 = 280.;
/// Below this the list and the detail stack instead of sitting side by side,
/// which is the Windows rule for a window this narrow.
pub const STACK_WIDTH: f32 = 641.;
/// Below this the right panel folds away: it holds what the client knows, and
/// the work needs the width more than the facts do.
pub const PANEL_WIDTH_MIN: f32 = 1000.;

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
/// the picker offers, the engine behind it, and the width that decides whether
/// the columns stack.
pub struct Chrome<'a> {
    /// The Project the work belongs to.
    pub project: Option<&'a str>,
    /// The Projects the picker offers.
    pub projects: &'a [Project],
    /// The engine this client is talking to.
    pub engine: EngineFacts,
    /// The window's width, which decides whether the columns stack.
    pub width: Pixels,
}

/// What a screen fills: its list column, its detail, and anything it has to say
/// about what is open, which the right panel shows.
pub struct Slots {
    pub list: AnyElement,
    pub detail: AnyElement,
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

    /// The chip that names the open Project. The screen puts it in its list
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

    /// The window: a band across the top, the columns under it.
    pub fn render(
        &self,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
        chrome: Chrome<'_>,
        slots: Slots,
    ) -> AnyElement {
        let narrow = chrome.width < px(STACK_WIDTH);
        // The panel holds what the client knows, and the work needs the width
        // more than the facts do, so the panel is the first thing to go.
        let shows_panel = !narrow && chrome.width >= px(PANEL_WIDTH_MIN);
        let Slots {
            list,
            detail,
            inspector,
        } = slots;

        let list_column = div()
            .v_flex()
            .w(px(LIST_WIDTH))
            .h_full()
            .bg(cx.theme().sidebar)
            .child(list);
        let detail_column = div()
            .v_flex()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .bg(cx.theme().background)
            .child(detail);
        // A narrow window stacks the list over the work rather than squeezing
        // both, which is the Windows rule for a window this size.
        let middle = if narrow {
            div()
                .v_flex()
                .flex_1()
                .min_w(px(0.))
                .child(
                    div()
                        .v_flex()
                        .h(px(180.))
                        .bg(cx.theme().sidebar)
                        .child(list_column),
                )
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

        let columns = div()
            .h_flex()
            .items_stretch()
            .flex_1()
            .min_h(px(0.))
            .child(self.rail(&chrome, cx))
            .child(divider(true, cx))
            .child(middle)
            .children(shows_panel.then(|| divider(true, cx)))
            .children(shows_panel.then(|| self.inspector(&chrome, inspector, cx)));

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
            .bg(cx.theme().background)
            .child(self.title_bar(window, cx))
            .child(columns)
            // The picker needs an overlay under it, and the overlay has to be
            // above the columns, so both come after the content they cover.
            .children(overlay)
            .children(panel)
            .into_any_element()
    }

    /// The band that carries the window controls. The corner above the rail
    /// keeps the rail's colour, so the navigation reaches the window's top edge.
    fn title_bar(&self, window: &mut Window, cx: &mut Context<DesktopApp>) -> AnyElement {
        let mut bar = TitleBar::new().pl(px(0.)).child(
            div()
                .h_flex()
                .flex_1()
                .h_full()
                .items_center()
                .child(div().w(px(RAIL_WIDTH)).h_full().bg(cx.theme().sidebar)),
        );
        if draws_own_controls(window) {
            bar = bar.child(window_controls(cx));
        }
        bar.into_any_element()
    }

    /// The destinations, each an icon and its name.
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
                .gap_2()
                .items_center()
                .px_2()
                .py_1()
                .rounded(px(ui::RADIUS))
                .child(section.icon().with_size(px(16.)).text_color(tone))
                .child(
                    div()
                        .text_style(&ui::BODY)
                        .text_color(tone)
                        .child(section.title()),
                )
                .tooltip({
                    let title = section.title();
                    move |window, cx| Tooltip::new(title).build(window, cx)
                });
            let row = if selected {
                row.bg(cx.theme().sidebar_accent)
            } else {
                row.hover(|this| this.bg(cx.theme().list_hover))
            };
            row.on_click(cx.listener(move |app, _event, _window, cx| {
                app.select_section(section, cx);
            }))
        });

        let engine = div()
            .id("engine-state")
            .h_flex()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .text_style(&ui::CAPTION)
            .text_color(cx.theme().muted_foreground)
            .hover(|this| this.text_color(cx.theme().foreground))
            .child(engine_dot(chrome, cx))
            .child(ui::truncate(
                if chrome.engine.connected {
                    &chrome.engine.version
                } else {
                    "engine unavailable"
                },
                18,
            ))
            .on_click(cx.listener(|app, _event, _window, cx| app.recheck_engine(cx)));

        div()
            .v_flex()
            .w(px(RAIL_WIDTH))
            .h_full()
            .p_2()
            .gap_1()
            .bg(cx.theme().sidebar)
            .children(rows)
            .child(div().flex_1())
            .child(engine)
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
            .p_3()
            .gap_4()
            .bg(cx.theme().sidebar);
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
        .size(px(8.))
        .rounded_full()
        .bg(color)
        .into_any_element()
}

/// A one-pixel rule between regions. The layout has no border widths for single
/// edges, so a rule is an element like any other.
fn divider(vertical: bool, cx: &App) -> AnyElement {
    let rule = div().bg(cx.theme().sidebar_border);
    if vertical {
        rule.w(px(1.)).into_any_element()
    } else {
        rule.h(px(1.)).into_any_element()
    }
}
