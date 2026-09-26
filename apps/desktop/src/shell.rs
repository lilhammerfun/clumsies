//! The window shell: what every screen shares.
//!
//! Read from the macOS client's WorkspaceView: its regularWorkspace is a
//! NavigationSplitView whose sidebar is GlobalSidebar (the six destinations),
//! whose content column is the current section's navigator, and whose detail is
//! the work itself. The Project filter, the document view picker and the
//! section's actions sit in the detail's toolbar there.
//!
//! This client draws the same three columns and separates chrome from work: the
//! section rail, the context bar and the status bar belong to the window, and a
//! screen fills exactly two slots, its list and its detail. Nothing else about a
//! screen's layout is its own business, which is what keeps Reviews, Inbox and
//! Settings from each inventing a window.
//!
//! The context bar is the one deliberate difference. macOS spreads what a window
//! is looking at across a toolbar filter, the sidebar header and the sidebar
//! footer; here it is one row naming the Project, the directory the Project is
//! bound to, where the daemon keeps it, and which checkout is open. Those are
//! what every action in the window applies to, so they are stated once, next to
//! the actions.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::engine::Project;
use crate::ui::{self, Typography};

/// The section rail, at the width macOS gives its sidebar.
pub const RAIL_WIDTH: f32 = 220.;
/// The same rail with labels dropped, which is what a narrow window gets.
pub const RAIL_COMPACT_WIDTH: f32 = 56.;
/// The list column, at the width macOS gives a section's navigator.
pub const LIST_WIDTH: f32 = 300.;
/// Below this the list and the detail stack instead of sitting side by side,
/// which is the Windows rule for a window this narrow.
pub const STACK_WIDTH: f32 = 641.;

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

    fn icon(self) -> IconName {
        match self {
            Section::Dashboard => IconName::LayoutDashboard,
            Section::Inbox => IconName::Inbox,
            Section::Memory => IconName::BookOpen,
            Section::Bundles => IconName::FolderClosed,
            Section::Reviews => IconName::Replace,
            Section::Activity => IconName::ChartPie,
        }
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

/// What the window supplies for its own chrome: the facts the context bar
/// names, the Project list its panel offers, the actions it places, and the
/// status line below. One value, because they are all decisions the window
/// makes about itself rather than anything a screen owns.
pub struct Chrome<'a> {
    /// The Project the work belongs to.
    pub project: Option<&'a str>,
    /// The directory the Project is bound to on this machine, if it is bound.
    pub workspace: Option<&'a str>,
    /// Where the daemon keeps this Project's state.
    pub storage: Option<&'a str>,
    /// The Project ref the open checkout resolved to.
    pub commit: Option<&'a str>,
    /// How many documents have a proposal waiting.
    pub drafts: usize,
    /// The Projects the chip's panel offers.
    pub projects: &'a [Project],
    /// The window's width, which decides whether the columns stack.
    pub width: Pixels,
}

/// The two slots a screen fills.
pub struct Slots {
    pub list: AnyElement,
    pub detail: AnyElement,
}

pub struct Shell {
    section: Section,
    /// The Project list is chrome, not a screen, so the shell owns whether its
    /// panel is open.
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

    /// The window, in three rows: the context bar, the columns, and the status
    /// bar. A narrow window drops the rail's labels and stacks the list over
    /// the detail.
    pub fn render(
        &self,
        cx: &mut Context<DesktopApp>,
        chrome: Chrome<'_>,
        slots: Slots,
        actions: AnyElement,
        status: AnyElement,
    ) -> AnyElement {
        let narrow = chrome.width < px(STACK_WIDTH);
        let middle = if narrow {
            div()
                .v_flex()
                .flex_1()
                .min_h(px(0.))
                .child(div().v_flex().h(px(200.)).child(slots.list))
                .child(divider(false, cx))
                .child(div().v_flex().flex_1().min_h(px(0.)).child(slots.detail))
                .into_any_element()
        } else {
            div()
                .h_flex()
                .items_stretch()
                .flex_1()
                .min_h(px(0.))
                .child(div().v_flex().w(px(LIST_WIDTH)).child(slots.list))
                .child(divider(true, cx))
                .child(div().v_flex().flex_1().min_w(px(0.)).child(slots.detail))
                .into_any_element()
        };

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
            .child(self.context_bar(&chrome, actions, narrow, cx))
            .child(divider(false, cx))
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.rail(narrow, cx))
                    .child(divider(true, cx))
                    .child(middle),
            )
            .child(divider(false, cx))
            .child(self.status_bar(status))
            // The panel needs an overlay under it, and the overlay has to be
            // above the columns, so both come after the content they cover.
            .children(overlay)
            .children(panel)
            .into_any_element()
    }

    /// The row that says what the actions in this window apply to.
    fn context_bar(
        &self,
        chrome: &Chrome<'_>,
        actions: AnyElement,
        narrow: bool,
        cx: &mut Context<DesktopApp>,
    ) -> AnyElement {
        let project = chrome.project.unwrap_or("No Project").to_owned();
        let project_chip = div()
            .id("project-chip")
            .h_flex()
            .gap_1()
            .items_center()
            .px_2()
            .py_1()
            .rounded(px(ui::RADIUS))
            .bg(cx.theme().list_hover)
            .child(
                Icon::new(IconName::BookOpen)
                    .with_size(px(14.))
                    .text_color(cx.theme().foreground),
            )
            .child(
                div()
                    .text_style(&ui::BODY)
                    .child(ui::truncate(&project, 28)),
            )
            .child(
                Icon::new(IconName::ChevronDown)
                    .with_size(px(14.))
                    .text_color(cx.theme().muted_foreground),
            )
            .on_click(cx.listener(|app, _event, _window, cx| app.toggle_projects(cx)));

        let mut bar = div()
            .h_flex()
            .h(px(40.))
            .px_3()
            .gap_3()
            .items_center()
            .child(project_chip);

        if !narrow {
            bar = bar
                .children(
                    chrome
                        .workspace
                        .and_then(ui::last_segment)
                        .map(|name| chip(IconName::FolderClosed, name, cx)),
                )
                .children(
                    chrome
                        .storage
                        .map(|storage| chip(IconName::HardDrive, storage.to_owned(), cx)),
                )
                .children(
                    chrome
                        .commit
                        .map(|commit| chip(IconName::Replace, ui::shorten(commit, 8), cx)),
                )
                .children((chrome.drafts > 0).then(|| {
                    div()
                        .h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Icon::new(IconName::CircleCheck)
                                .with_size(px(14.))
                                .text_color(cx.theme().primary),
                        )
                        .child(
                            div()
                                .text_style(&ui::CAPTION)
                                .text_color(cx.theme().primary)
                                .child(match chrome.drafts {
                                    1 => "1 draft".to_owned(),
                                    count => format!("{count} drafts"),
                                }),
                        )
                        .into_any_element()
                }));
        }

        bar.child(div().flex_1()).child(actions).into_any_element()
    }

    /// The six destinations. macOS puts the organization above them and the
    /// account below; this client names the Server in the rail and reports the
    /// engine in the status bar.
    fn rail(&self, narrow: bool, cx: &mut Context<DesktopApp>) -> AnyElement {
        let rows = Section::ALL.into_iter().map(|section| {
            let selected = section == self.section;
            let tone = if selected {
                cx.theme().foreground
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
                .child(
                    Icon::new(section.icon())
                        .with_size(px(16.))
                        .text_color(tone),
                )
                .children((!narrow).then(|| {
                    div()
                        .text_style(&ui::BODY)
                        .text_color(tone)
                        .child(section.title())
                }));
            let row = if selected {
                row.bg(cx.theme().list_active)
            } else {
                row.hover(|this| this.bg(cx.theme().list_hover))
            };
            row.on_click(cx.listener(move |app, _event, _window, cx| {
                app.select_section(section, cx);
            }))
        });

        div()
            .v_flex()
            .w(px(if narrow {
                RAIL_COMPACT_WIDTH
            } else {
                RAIL_WIDTH
            }))
            .p_2()
            .gap_1()
            .children(rows)
            .child(div().flex_1())
            .into_any_element()
    }

    fn status_bar(&self, status: AnyElement) -> AnyElement {
        div()
            .h_flex()
            .h(px(24.))
            .px_3()
            .gap_3()
            .items_center()
            .child(div().flex_1().min_w(px(0.)).child(status))
            .into_any_element()
    }

    /// The Project list, under the chip that names the current Project. macOS
    /// keeps this list in the sidebar; this client shows it where the Project is
    /// named, because the sidebar is the section list here.
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
            .top(px(44.))
            .left(px(ui::SPACE_MD))
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

/// One fact about the window, in the context bar.
fn chip(icon: IconName, label: String, cx: &App) -> AnyElement {
    div()
        .h_flex()
        .gap_1()
        .items_center()
        .child(
            Icon::new(icon)
                .with_size(px(14.))
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .into_any_element()
}
