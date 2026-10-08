//! The Dashboard: what the Project publishes, and what its retrieval did.
//!
//! Read from the macOS client's `DashboardPage` and `DashboardView`. The numbers
//! have two owners and are joined here: the Server counts the published
//! inventory and owns the calendar, and the engine reports the retrieval it
//! retained on this machine. The page is macOS's shape: the six metric cards
//! across the top — six to a row while there is room, three when there is not —
//! and the six panels below them, two to a row while the page has the width for
//! it. macOS has no navigator beside this screen (its split view is the sidebar
//! next to one page), so the Project filter travels in this page's own header,
//! which is where macOS keeps it too; the period picker macOS keeps in its
//! window toolbar is in that same header, beside the region it scopes.
//!
//! Each panel also names the peak its chart is scaled by. macOS draws y-axis
//! marks; this client's charts are columns without an axis, so the scale is
//! said in words rather than left for the reader to infer.
//!
//! Hovering a chart draws the rule macOS draws at that day and the day's own
//! numbers beside it — its `RuleMark` and the annotation it carries. Which day
//! the pointer is over is found with one invisible cell per day laid over the
//! plot rather than by measuring the pointer, so the day a reader is on is the
//! day the chart marks whatever the widths work out to.
//!
//! Not built, and named here rather than left to be discovered: the component
//! library's chart element, which draws one series per value and cannot stack a
//! day's outcomes, so the columns here are painted instead; and the
//! organization-wide scope, because this client has one Project rather than an
//! Organization view. A Dev Instance's sample is read, as macOS reads it, and
//! badged so a sample is never mistaken for the Project's own history.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::header;
use crate::engine::{
    AgentUsage, AgentUsageDay, DashboardBar, DashboardSnapshot, DeltaDay, DeltaStatistics, Period,
    RetrievalStatistics,
};
use crate::timestamps;
use crate::ui::{self, Typography};

/// How tall a panel's chart is.
const CHART_HEIGHT: f32 = 140.;
/// The label column of a horizontal bar chart, which macOS fixes at 150pt.
const BAR_LABEL_WIDTH: f32 = 150.;
/// Two panels to a row while the pane is at least this wide, which is macOS's
/// own 940pt threshold.
const TWO_COLUMN_WIDTH: f32 = 940.;
/// Six metric cards to a row while the page is at least this wide, and three
/// when it is not. macOS states 1100pt for six; its cards carry 10pt captions
/// and this client's floor is 12px, so the same six cards need a fifth more
/// room before a card's own line stops being cut.
const CARD_ROW_WIDTH: f32 = 1320.;

/// One panel of the Dashboard, in the order macOS draws them. The title,
/// subtitle, footnote and explanation are its `DashboardMetric` table,
/// verbatim, because they are what the product says these numbers mean.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Growth,
    Retrieval,
    Directories,
    Top,
    Delta,
    AgentUsage,
}

impl Metric {
    const ALL: [Metric; 6] = [
        Metric::Growth,
        Metric::Retrieval,
        Metric::Directories,
        Metric::Top,
        Metric::Delta,
        Metric::AgentUsage,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Metric::Growth => "Memory growth",
            Metric::Retrieval => "Retrieval activity",
            Metric::Directories => "Directory distribution & coverage",
            Metric::Top => "Frequently retrieved memories",
            Metric::Delta => "Delta retrieval reuse",
            Metric::AgentUsage => "Agent Memory usage",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Metric::Growth => "Published memory at the end of each day",
            Metric::Retrieval => "Completed retrieval requests on this Mac",
            Metric::Directories => "Published document count and retrieval coverage",
            Metric::Top => "One count per document per retrieval",
            Metric::Delta => "Reuse / (Add + Replace + Reuse)",
            Metric::AgentUsage => "Completed main-agent runs · Codex on this Mac",
        }
    }

    fn footnote(self) -> &'static str {
        match self {
            Metric::Growth => "Document updates do not increase inventory",
            Metric::Retrieval => "With content includes reused fragments",
            Metric::Directories => "Coverage counts each document once within the selected period",
            Metric::Top => "Click a document to open Memory",
            Metric::Delta => "State supplied does not guarantee matching fragments",
            Metric::AgentUsage => "Calls include activate, load and store, even when they fail",
        }
    }

    /// How the panel's numbers are arrived at, which macOS shows in a sheet
    /// opened from the panel's own "About".
    pub fn explanation(self) -> &'static str {
        match self {
            Metric::Growth => {
                "The server calculates daily closing inventory from published commits. Project scope includes selected organization memories; selection changes affect its count. Draft overlays are excluded. Days before the first retained commit remain unknown."
            }
            Metric::Retrieval => {
                "Completed memory.activate requests, separated into returned content, successful empty results, and failures. Reused fragments count as content. Requests still running are excluded. Retained local traces may not cover the entire period; these counts do not measure agent adoption or accuracy."
            }
            Metric::Directories => {
                "Published documents are grouped by directory. When all documents share a parent directory, its subdirectories are shown. The light bar shows current inventory; the blue portion shows distinct current documents retrieved within the selected period. Unpublished drafts are excluded."
            }
            Metric::Top => {
                "Current documents ranked by successful retrievals in the selected period. Multiple fragments from one document count once per request, including reused fragments. The six most frequently retrieved documents are shown."
            }
            Metric::Delta => {
                "Reuse is an unchanged selected fragment already represented in the caller's state. Add is absent from that state, not necessarily newly created memory. Replace has a changed content hash. The ratio counts recorded fragments in successful retained retrievals, including drafts; missing actions and empty results do not count as zero reuse. State supplied counts all completed requests carrying a token, including invalid tokens. Period totals are weighted by fragment counts."
            }
            Metric::AgentUsage => {
                "A run is a native Codex turn with both start and terminal events, grouped by start date. Repeated calls count once per run; calls that fail still count as usage. Ongoing runs, subagents, other hosts, and logs without reliable boundaries are excluded. Only local sessions belonging to the selected projects are observed. Missing logs cannot establish non-use. This measures calling Memory, not answer quality or whether retrieved content was adopted."
            }
        }
    }

    /// Whether macOS's note about the local retention ceiling belongs to this
    /// panel's explanation.
    fn explains_local_history(self) -> bool {
        matches!(
            self,
            Metric::Retrieval | Metric::Directories | Metric::Top | Metric::Delta
        )
    }
}

/// The colours a panel draws with: the one its series are drawn in, and the
/// semantic colours a stacked or share chart is split into.
#[derive(Clone, Copy)]
struct Palette {
    series: Hsla,
    muted: Hsla,
    success: Hsla,
    warning: Hsla,
    danger: Hsla,
}

/// The Dashboard's own screen: the period the reader is looking at, and the
/// snapshot the two owners answered with.
pub struct DashboardScreen {
    /// The Project whose statistics these are. Another Project is another read.
    project_id: Option<String>,
    period: Period,
    snapshot: Option<DashboardSnapshot>,
    loading: bool,
    /// Why there is nothing to show at all.
    error: Option<String>,
    /// Why the newest read failed while an older snapshot is still on screen.
    /// macOS keeps showing what it has and says so, rather than blanking the
    /// page the reader is reading.
    notice: Option<String>,
    /// Which read an answer belongs to. A period the reader has left must not
    /// paint its answer over the one they are waiting for.
    generation: u64,
    /// Which day of which chart the pointer is over. macOS draws a rule at that
    /// day and the day's numbers beside it, which is what a reader hovering a
    /// chart is asking for.
    hover: Option<(Metric, usize)>,
}

impl DashboardScreen {
    pub fn new(_cx: &mut Context<DesktopApp>) -> Self {
        Self {
            project_id: None,
            period: Period::Month,
            snapshot: None,
            loading: false,
            error: None,
            notice: None,
            generation: 0,
            hover: None,
        }
    }

    /// The pointer entered or left one day of one chart. Leaving a day clears
    /// the hover only while it is still that day's: a pointer moving from one
    /// column to the next reports the leave after the enter, and the day it
    /// landed on is the one to keep.
    pub fn set_chart_hover(
        &mut self,
        metric: Metric,
        slot: usize,
        over: bool,
        cx: &mut Context<DesktopApp>,
    ) {
        let next = if over {
            Some((metric, slot))
        } else if self.hover == Some((metric, slot)) {
            None
        } else {
            return;
        };
        if self.hover != next {
            self.hover = next;
            cx.notify();
        }
    }

    /// Which day of one chart the pointer is over, when it is over one.
    fn hovered_slot(&self, metric: Metric, slots: usize) -> Option<usize> {
        match self.hover {
            Some((hovered, slot)) if hovered == metric && slot < slots => Some(slot),
            _ => None,
        }
    }

    /// The Project this Dashboard is about. Another Project empties it rather
    /// than showing one Project's numbers under another's name.
    pub fn set_project(&mut self, project_id: Option<String>, cx: &mut Context<DesktopApp>) {
        if self.project_id == project_id {
            return;
        }
        self.generation += 1;
        self.project_id = project_id;
        self.snapshot = None;
        self.error = None;
        self.notice = None;
        self.loading = false;
        cx.notify();
    }

    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    pub fn period(&self) -> Period {
        self.period
    }

    /// Whether the screen has nothing to draw and a Project to ask about.
    pub fn needs_reading(&self) -> bool {
        self.snapshot.is_none() && !self.loading && self.project_id.is_some()
    }

    /// Starts a read, and returns the stamp its answer has to carry back.
    pub fn begin_read(&mut self, period: Period) -> u64 {
        self.period = period;
        self.loading = true;
        self.error = None;
        self.notice = None;
        self.generation += 1;
        self.generation
    }

    /// Takes an answer, if it belongs to the read the reader is still waiting
    /// for.
    pub fn set_snapshot(
        &mut self,
        generation: u64,
        result: Result<DashboardSnapshot, String>,
        cx: &mut Context<DesktopApp>,
    ) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                self.error = None;
                self.notice = None;
            }
            Err(error) if self.snapshot.is_some() => self.notice = Some(error),
            Err(error) => {
                self.snapshot = None;
                self.error = Some(error);
            }
        }
        cx.notify();
    }

    /// Whether the reader asked for a period they are not already looking at.
    pub fn set_period(&mut self, period: Period) -> bool {
        if self.period == period {
            return false;
        }
        self.period = period;
        // The answer for one period is not an answer for another, so the pane
        // goes back to waiting rather than showing one period's charts under
        // another period's name.
        self.snapshot = None;
        self.error = None;
        self.notice = None;
        true
    }

    /// How many requests per Project the engine keeps, which the About sheet
    /// names when it explains where the local numbers come from.
    pub fn retention_per_project(&self) -> Option<i64> {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.retrieval.retention_per_project)
    }

    /// The page: the metric cards macOS draws across the top, then the panels,
    /// under one header row that carries the Project filter and the period.
    ///
    /// The row has no title, because macOS's page has none: the destination is
    /// named by the rail the reader came through. macOS has no navigator column
    /// for the Dashboard either — its split view is the sidebar beside this one
    /// page — so the Project filter lives here, in the page's own header, which
    /// is where macOS keeps it in its toolbar. The page is pointer-first, as
    /// macOS's is: no region of this client's keyboard walk belongs to it.
    pub fn detail(
        &self,
        project: AnyElement,
        actions: Option<AnyElement>,
        width: Pixels,
        cx: &mut Context<DesktopApp>,
    ) -> AnyElement {
        let header = header::row()
            .child(project)
            .child(div().flex_1())
            .children(actions);

        div()
            .v_flex()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(header)
            .child(ui::rule(cx))
            .child(self.body(width, cx))
            .into_any_element()
    }

    /// The panels, or what stands in their place.
    fn body(&self, width: Pixels, cx: &mut Context<DesktopApp>) -> AnyElement {
        let Some(snapshot) = &self.snapshot else {
            return self.standing_in(cx);
        };
        let muted = cx.theme().muted_foreground;
        let mut children: Vec<AnyElement> = Vec::new();
        // macOS marks the same sample the same way: numbers taken from a Dev
        // Instance's fixture are not this Project's own history.
        if snapshot.demo {
            children.push(demo_badge(cx));
        }
        if snapshot.stale {
            children.push(ui::message(
                "Showing cached server statistics. Updates will resume when the server is available.",
                muted,
            ));
        }
        if self.notice.is_some() {
            children.push(ui::message(
                "Showing previous data. Updates will resume automatically.",
                muted,
            ));
        }

        // macOS's `metrics(_:width:)`: six cards across while there is room for
        // them, three when there is not.
        let facts = card_facts(snapshot);
        let per_card_row = if width >= px(CARD_ROW_WIDTH) { 6 } else { 3 };
        for row in facts.chunks(per_card_row) {
            let cards = row.iter().map(|(title, value, detail, symbol)| {
                card(title, *value, detail.clone(), symbol, cx)
            });
            children.push(
                div()
                    .h_flex()
                    .items_stretch()
                    .gap_3()
                    .children(cards)
                    .into_any_element(),
            );
        }

        let per_row = if width >= px(TWO_COLUMN_WIDTH) { 2 } else { 1 };
        for chunk in Metric::ALL.chunks(per_row) {
            let panels = chunk
                .iter()
                .map(|metric| self.panel(*metric, snapshot, cx))
                .collect::<Vec<_>>();
            children.push(
                div()
                    .h_flex()
                    .items_stretch()
                    .gap_3()
                    .children(panels)
                    .into_any_element(),
            );
        }

        div()
            .id("dashboard-panels")
            .v_flex()
            .flex_1()
            .min_h(px(0.))
            .gap_3()
            .p_4()
            .overflow_y_scroll()
            .children(children)
            .into_any_element()
    }

    /// What the detail says when there is no snapshot: a read in flight, a
    /// failure with the engine's own sentence, or a Project to choose.
    fn standing_in(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let (muted, danger) = {
            let theme = cx.theme();
            (theme.muted_foreground, theme.danger)
        };
        let mut column = div().v_flex().items_start().gap_2();
        if let Some(error) = &self.error {
            column = column
                .child(
                    div()
                        .text_style(&ui::BODY)
                        .text_color(cx.theme().foreground)
                        .child("Dashboard unavailable"),
                )
                .child(ui::message(error.clone(), danger))
                .child(
                    header::button("dashboard-retry")
                        .icon(Icon::default().path("icons/rotate-cw.svg"))
                        .label("Retry")
                        .on_click(
                            cx.listener(|app, _event, _window, cx| app.refresh_dashboard(cx)),
                        ),
                );
        } else if self.loading {
            column = column.child(ui::message("Loading dashboard…", muted));
        } else if self.project_id.is_none() {
            column = column.child(ui::message("Choose a Project to see its Dashboard.", muted));
        } else {
            // A pane is never left blank without saying why: this is the state
            // between a Project being picked and its statistics being asked
            // for.
            column = column.child(ui::message("No statistics yet.", muted));
        }
        div()
            .v_flex()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .p_4()
            .child(column)
            .into_any_element()
    }

    /// One panel: what it is, the chart the numbers make, and what they mean.
    fn panel(
        &self,
        metric: Metric,
        snapshot: &DashboardSnapshot,
        cx: &mut Context<DesktopApp>,
    ) -> AnyElement {
        let (foreground, muted, border, surface) = {
            let theme = cx.theme();
            (
                theme.foreground,
                theme.muted_foreground,
                theme.border,
                theme.group_box,
            )
        };
        let content = self.content(metric, snapshot, cx);
        let this = cx.entity();
        div()
            .v_flex()
            .flex_1()
            .min_w(px(0.))
            .gap_3()
            .p_4()
            .rounded(px(ui::RADIUS_LG))
            .border_1()
            .border_color(border)
            .bg(surface)
            .child(
                div()
                    .h_flex()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap_1()
                            .child(
                                div()
                                    .text_style(&ui::BODY)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(foreground)
                                    .child(metric.title()),
                            )
                            .child(
                                div()
                                    .text_style(&ui::CAPTION)
                                    .text_color(muted)
                                    .child(metric.subtitle()),
                            ),
                    )
                    .children(content.context.map(|context| {
                        div()
                            .flex_shrink_0()
                            .text_style(&ui::CAPTION)
                            .text_color(muted)
                            .child(context)
                    })),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .justify_end()
                    .child(content.element),
            )
            .child(ui::rule(cx))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_style(&ui::CAPTION)
                            .text_color(muted)
                            .child(metric.footnote()),
                    )
                    .child({
                        let title = metric.title();
                        // The mark alone: a panel's own explanation is a rare
                        // command, and a word beside every one of them is noise.
                        // The tooltip still says what the mark opens.
                        header::button(("dashboard-about", metric as usize))
                            .icon(Icon::default().path("icons/info.svg"))
                            .tooltip(format!("How {title} is calculated"))
                            .on_click(move |_event, window, cx| {
                                this.update(cx, |app, cx| {
                                    app.open_dashboard_about(metric, window, cx)
                                });
                            })
                    }),
            )
            .into_any_element()
    }

    /// What one panel draws: the chart, and the fact the chart is scaled by.
    fn content(
        &self,
        metric: Metric,
        snapshot: &DashboardSnapshot,
        cx: &mut Context<DesktopApp>,
    ) -> Content {
        let theme = cx.theme();
        // The series colour is the theme's blue and not `primary`: in dark mode
        // `primary` is the foreground's own colour, which would draw a chart in
        // white. macOS draws these series in blue, green, amber and red.
        let palette = Palette {
            series: theme.chart_3,
            muted: theme.muted_foreground,
            success: theme.success,
            warning: theme.warning,
            danger: theme.danger,
        };
        let Palette {
            series,
            warning,
            danger,
            ..
        } = palette;
        match metric {
            Metric::Growth => {
                let days = &snapshot.memory.days;
                // macOS plots one point per day the Server could count, in that
                // day's place on the axis, and joins them: a running total is a
                // line, not a wall of columns.
                let points: Vec<(usize, usize)> = days
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, day)| day.memory_count.map(|count| (slot, count)))
                    .collect();
                if points.is_empty() {
                    return Content::empty(
                        "No inventory history yet",
                        "Historical counts require recorded snapshots.",
                        cx,
                    );
                }
                let peak = snapshot.peak_inventory().max(1);
                let plot = inventory_chart(days.len(), &points, peak, CHART_HEIGHT, series, cx);
                let hovered = self
                    .hovered_slot(Metric::Growth, days.len())
                    .filter(|slot| days[*slot].memory_count.is_some());
                let tip = hovered.map(|slot| {
                    day_tip(
                        timestamps::day(days[slot].date),
                        format!(
                            "{} memories",
                            thousands(days[slot].memory_count.unwrap_or(0))
                        ),
                        cx,
                    )
                });
                Content::drawn(
                    Some(format!("peak {}", thousands(snapshot.peak_inventory()))),
                    with_hover(Metric::Growth, days.len(), hovered, tip, plot, cx),
                    period_labels(snapshot),
                    Vec::new(),
                    cx,
                )
            }
            Metric::Retrieval => {
                let days = &snapshot.retrieval.days;
                if days.is_empty() {
                    return Content::empty(
                        "No retrieval records",
                        "Retrieval activity will appear here.",
                        cx,
                    );
                }
                let peak = days
                    .iter()
                    .map(|day| day.returned + day.empty + day.failed)
                    .max()
                    .unwrap_or(0);
                let columns: Vec<Vec<(usize, Hsla)>> = snapshot
                    .memory
                    .days
                    .iter()
                    .map(|day| {
                        days.iter()
                            .find(|retrieval| retrieval.date == day.date)
                            .map_or_else(Vec::new, |retrieval| {
                                vec![
                                    (retrieval.returned, series),
                                    (retrieval.empty, warning),
                                    (retrieval.failed, danger),
                                ]
                            })
                    })
                    .collect();
                let slots = snapshot.memory.days.len();
                let plot = columns_chart(&columns, Some(peak), CHART_HEIGHT, cx);
                let hovered = self
                    .hovered_slot(Metric::Retrieval, slots)
                    .filter(|slot| columns[*slot].iter().any(|(value, _)| *value > 0));
                let tip = hovered.and_then(|slot| {
                    let date = snapshot.memory.days[slot].date;
                    days.iter()
                        .find(|retrieval| retrieval.date == date)
                        .map(|retrieval| {
                            day_tip(
                                timestamps::day(date),
                                format!(
                                    "{} returned · {} empty · {} failed",
                                    thousands(retrieval.returned),
                                    thousands(retrieval.empty),
                                    thousands(retrieval.failed)
                                ),
                                cx,
                            )
                        })
                });
                Content::drawn(
                    // A flat week can mean a quiet week or a week this machine
                    // never recorded, so where the local history starts travels
                    // with the chart that is drawn from it.
                    Some(match snapshot.retrieval.history_start {
                        Some(start) => format!(
                            "peak {} requests · kept since {}",
                            thousands(peak),
                            timestamps::day(start)
                        ),
                        None => format!("peak {} requests", thousands(peak)),
                    }),
                    with_hover(Metric::Retrieval, slots, hovered, tip, plot, cx),
                    period_labels(snapshot),
                    vec![
                        ("With content", series),
                        ("Empty", warning),
                        ("Failed", danger),
                    ],
                    cx,
                )
            }
            Metric::Directories => {
                let rows = directory_rows(&snapshot.retrieval);
                if rows.is_empty() {
                    return Content::empty(
                        "No memory yet",
                        "Select memory for this project to see its distribution.",
                        cx,
                    );
                }
                Content::bars(rows, true, None, cx)
            }
            Metric::Top => {
                let rows = snapshot.retrieval.top_resources.clone();
                if rows.is_empty() {
                    return Content::empty(
                        "No retrieved memories",
                        "Documents returned by retrieval will appear here.",
                        cx,
                    );
                }
                Content::bars(rows, false, Some(snapshot), cx)
            }
            Metric::Delta => {
                let Some(delta) = &snapshot.retrieval.delta else {
                    return Content::empty(
                        "Delta statistics unavailable",
                        "Update the local runtime to collect these statistics.",
                        cx,
                    );
                };
                delta_panel(delta, snapshot, palette, self, cx)
            }
            Metric::AgentUsage => {
                let Some(usage) = &snapshot.retrieval.agent_usage else {
                    return Content::empty(
                        "Agent usage unavailable",
                        "Update the local runtime to collect these statistics.",
                        cx,
                    );
                };
                agent_panel(usage, snapshot, palette, self, cx)
            }
        }
    }
}

/// A chart and its hover: the plot, one invisible cell per day so the pointer's
/// day is known without measuring anything, and — while a day with something to
/// say is under the pointer — the rule macOS draws through it and that day's
/// numbers, which is its `RuleMark` and the annotation beside it.
fn with_hover(
    metric: Metric,
    slots: usize,
    hovered: Option<usize>,
    tip: Option<AnyElement>,
    plot: AnyElement,
    cx: &mut Context<DesktopApp>,
) -> AnyElement {
    let mut chart = div().relative().w_full().child(plot);
    if let Some(slot) = hovered {
        let at = (slot as f32 + 0.5) / slots.max(1) as f32;
        chart = chart.child(
            div()
                .absolute()
                .top_0()
                .left(relative(at))
                .w(px(1.))
                .h_full()
                .bg(cx.theme().muted_foreground),
        );
        if let Some(tip) = tip {
            // The tip is anchored at the day and kept off the right edge, which
            // is macOS's `overflowResolution: .fit(to: .chart)`.
            chart = chart.child(
                div()
                    .absolute()
                    .top(px(0.))
                    .left(relative(at.min(0.72)))
                    .child(tip),
            );
        }
    }
    let cells = (0..slots).map(|slot| {
        div()
            .id(("chart-day", metric as usize * 10_000 + slot))
            .flex_1()
            .h_full()
            .on_hover(cx.listener(move |app, over: &bool, _window, cx| {
                app.dashboard_hover(metric, slot, *over, cx);
            }))
            .into_any_element()
    });
    chart
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .h_flex()
                .children(cells),
        )
        .into_any_element()
}

/// One day's numbers, as macOS's chart annotation shows them: the date in the
/// quiet colour, then what the chart's marks mean for that day.
fn day_tip(date: String, text: String, cx: &App) -> AnyElement {
    let (surface, border, muted, foreground) = {
        let theme = cx.theme();
        (
            theme.popover,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
        )
    };
    div()
        .v_flex()
        .gap_1()
        .px_2()
        .py_1()
        .rounded(px(ui::RADIUS))
        .border_1()
        .border_color(border)
        .bg(surface)
        .child(div().text_style(&ui::CAPTION).text_color(muted).child(date))
        .child(
            div()
                .text_style(&ui::CAPTION)
                .font_weight(FontWeight::MEDIUM)
                .text_color(foreground)
                .child(text),
        )
        .into_any_element()
}

/// The dates a chart's axis is drawn under: the whole period the reader asked
/// for, which is the domain macOS scales its own charts to. A run of days with
/// nothing in it keeps its place rather than being closed up, so the shape of
/// the period is what the reader reads.
fn period_labels(snapshot: &DashboardSnapshot) -> (String, String, String) {
    labels(
        &snapshot
            .memory
            .days
            .iter()
            .map(|day| day.date)
            .collect::<Vec<_>>(),
    )
}

/// What one panel draws, and the fact printed beside its title: a chart, a set
/// of bars, or the reason there is neither.
struct Content {
    element: AnyElement,
    /// What the chart is scaled by, printed beside the panel's title.
    context: Option<String>,
}

impl Content {
    fn empty(title: &'static str, detail: &'static str, cx: &App) -> Self {
        Self {
            element: empty_panel(title, detail, cx),
            context: None,
        }
    }

    /// A panel whose chart is drawn for it, under the dates the reader asked
    /// for and, where the chart has one, the key to its colours.
    fn drawn(
        context: Option<String>,
        plot: AnyElement,
        labels: (String, String, String),
        legend: Vec<(&'static str, Hsla)>,
        cx: &App,
    ) -> Self {
        Self {
            element: div()
                .v_flex()
                .gap_2()
                .child(plot)
                .child(axis(labels, cx))
                .children(chart_legend(&legend, cx))
                .into_any_element(),
            context,
        }
    }

    /// Horizontal bars, which are labels and lengths rather than a chart.
    fn bars(
        rows: Vec<DashboardBar>,
        coverage: bool,
        open_in_memory: Option<&DashboardSnapshot>,
        cx: &mut Context<DesktopApp>,
    ) -> Self {
        Self {
            element: bar_chart(rows, coverage, open_in_memory, cx),
            context: None,
        }
    }
}

/// One metric card: what it is, what it says, and what that number counts.
/// macOS's `metric(_:value:detail:symbol:)`.
fn card(title: &str, value: usize, detail: String, symbol: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .v_flex()
        .flex_1()
        .min_w(px(0.))
        .gap_3()
        .p_4()
        .rounded(px(ui::RADIUS_LG))
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_style(&ui::CAPTION)
                        .text_color(theme.muted_foreground)
                        .child(title.to_owned()),
                )
                .child(
                    Icon::default()
                        .path(symbol)
                        .with_size(px(14.))
                        .flex_shrink_0()
                        .text_color(theme.chart_3),
                ),
        )
        .child(
            div()
                .text_style(&ui::TITLE)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(thousands(value)),
        )
        .child(
            div()
                .truncate()
                .text_style(&ui::CAPTION)
                .text_color(theme.muted_foreground)
                .child(detail),
        )
        .into_any_element()
}

/// The six cards of one snapshot, in macOS's order: what each one is called,
/// what it counts, what that number is measured over, and its symbol. They are
/// facts rather than elements so that the page can lay them out in rows of six
/// or three, which is what macOS's grid does.
#[allow(clippy::type_complexity)]
fn card_facts(snapshot: &DashboardSnapshot) -> Vec<(&'static str, usize, String, &'static str)> {
    let memory = &snapshot.memory;
    let retrieval = &snapshot.retrieval;
    let period = format!("Last {} days", snapshot.period.days());
    let coverage = if memory.resources.is_empty() {
        "No memory yet".to_owned()
    } else {
        format!("{} of current memory", percent(retrieval.coverage))
    };
    let drafts = (memory.open_drafts + memory.submitted_drafts).max(0) as usize;
    vec![
        (
            "Published memory",
            memory.memory_count,
            "Current total".to_owned(),
            "icons/brain.svg",
        ),
        (
            "New memories",
            memory.added_count,
            period.clone(),
            "icons/file-plus.svg",
        ),
        (
            "Updated memories",
            memory.updated_count,
            period,
            "icons/square-pen.svg",
        ),
        (
            "Retrievals",
            retrieval.retrievals,
            "On this Mac".to_owned(),
            "icons/search.svg",
        ),
        (
            "Memories retrieved",
            retrieval.recalled_count,
            coverage,
            "icons/text-search.svg",
        ),
        (
            "Synced drafts",
            drafts,
            format!(
                "{} open · {} in review",
                memory.open_drafts.max(0),
                memory.submitted_drafts.max(0)
            ),
            "icons/file-text.svg",
        ),
    ]
}

/// The directory bars, with everything past the fifth folded into one row so
/// that the chart stays readable — macOS's own rule.
fn directory_rows(retrieval: &RetrievalStatistics) -> Vec<DashboardBar> {
    let all = &retrieval.directories;
    if all.len() <= 6 {
        return all.clone();
    }
    let mut rows: Vec<DashboardBar> = all.iter().take(5).cloned().collect();
    let rest = &all[5..];
    rows.push(DashboardBar {
        id: "other-directories".to_owned(),
        label: "Other directories".to_owned(),
        value: rest.iter().map(|row| row.value).sum(),
        total: Some(rest.iter().map(|row| row.total.unwrap_or(0)).sum()),
    });
    rows
}

/// The delta panel: the reuse rate, what it was counted over, and the daily
/// shares behind it.
fn delta_panel(
    delta: &DeltaStatistics,
    snapshot: &DashboardSnapshot,
    palette: Palette,
    screen: &DashboardScreen,
    cx: &mut Context<DesktopApp>,
) -> Content {
    let Palette {
        series,
        warning,
        success,
        muted,
        ..
    } = palette;
    let mut headline = div().v_flex().gap_1().child(
        div()
            .h_flex()
            .items_baseline()
            .gap_3()
            .child(
                div()
                    .text_style(&ui::BODY_LARGE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(match delta.reuse_rate() {
                        Some(rate) => percent(rate),
                        None => "—".to_owned(),
                    }),
            )
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(muted)
                    .child(format!(
                        "{} / {} fragments",
                        thousands(delta.reused()),
                        thousands(delta.total())
                    )),
            ),
    );
    if snapshot.retrieval.retrievals > 0 {
        headline = headline.child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(muted)
                .child(format!(
                    "State supplied: {} · {} / {} requests",
                    percent(delta.with_state as f64 / snapshot.retrieval.retrievals as f64),
                    thousands(delta.with_state),
                    thousands(snapshot.retrieval.retrievals)
                )),
        );
    }
    if delta.unknown() > 0 {
        headline = headline.child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(muted)
                .child(format!(
                    "{} fragments have no recorded delta action",
                    thousands(delta.unknown())
                )),
        );
    }

    let days: Vec<&DeltaDay> = delta.days.iter().filter(|day| day.total() > 0).collect();
    if days.is_empty() {
        return Content {
            element: div()
                .v_flex()
                .h_full()
                .justify_end()
                .gap_2()
                .child(headline.into_any_element())
                .child(empty_panel(
                    "No delta samples",
                    "No recorded fragment actions in this period.",
                    cx,
                ))
                .into_any_element(),
            context: None,
        };
    }

    let columns: Vec<Vec<(usize, Hsla)>> = snapshot
        .memory
        .days
        .iter()
        .map(|day| {
            delta
                .days
                .iter()
                .find(|recorded| recorded.date == day.date && recorded.total() > 0)
                .map_or_else(Vec::new, |recorded| {
                    vec![
                        (recorded.added, series),
                        (recorded.replaced, warning),
                        (recorded.reused, success),
                    ]
                })
        })
        .collect();
    let slots = snapshot.memory.days.len();
    let plot = columns_chart(&columns, None, CHART_HEIGHT, cx);
    let hovered = screen
        .hovered_slot(Metric::Delta, slots)
        .filter(|slot| columns[*slot].iter().any(|(value, _)| *value > 0));
    let tip = hovered.and_then(|slot| {
        let date = snapshot.memory.days[slot].date;
        delta
            .days
            .iter()
            .find(|recorded| recorded.date == date && recorded.total() > 0)
            .map(|recorded| {
                day_tip(
                    timestamps::day(date),
                    format!(
                        "Add {} · Replace {} · Reuse {}",
                        thousands(recorded.added),
                        thousands(recorded.replaced),
                        thousands(recorded.reused)
                    ),
                    cx,
                )
            })
    });
    Content::drawn(
        None,
        with_hover(Metric::Delta, slots, hovered, tip, plot, cx),
        period_labels(snapshot),
        vec![
            ("Add · Newly provided", series),
            ("Replace · Updated", warning),
            ("Reuse · Already available", success),
        ],
        cx,
    )
    .with_headline(headline.into_any_element())
}

/// The agent-usage panel: how many runs called Memory, and the daily shares
/// behind it.
fn agent_panel(
    usage: &AgentUsage,
    snapshot: &DashboardSnapshot,
    palette: Palette,
    screen: &DashboardScreen,
    cx: &mut Context<DesktopApp>,
) -> Content {
    let Palette { series, muted, .. } = palette;
    let mut headline = div().v_flex().gap_1().child(
        div()
            .h_flex()
            .items_baseline()
            .gap_3()
            .child(
                div()
                    .text_style(&ui::BODY_LARGE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(match usage.usage_rate() {
                        Some(rate) => percent(rate),
                        None => "—".to_owned(),
                    }),
            )
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(muted)
                    .child(format!(
                        "{} / {} runs",
                        thousands(usage.with_memory()),
                        thousands(usage.total())
                    )),
            ),
    );
    if usage.excluded_sessions() > 0 {
        headline = headline.child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(muted)
                .child(format!(
                    "{} session logs excluded: incomplete or missing run boundaries",
                    thousands(usage.excluded_sessions())
                )),
        );
    }

    let days: Vec<&AgentUsageDay> = usage.days.iter().filter(|day| day.total() > 0).collect();
    if days.is_empty() {
        return Content {
            element: div()
                .v_flex()
                .h_full()
                .justify_end()
                .gap_2()
                .child(headline.into_any_element())
                .child(empty_panel(
                    "No observed agent runs",
                    "Completed local Codex runs with recorded boundaries will appear here.",
                    cx,
                ))
                .into_any_element(),
            context: None,
        };
    }

    let columns: Vec<Vec<(usize, Hsla)>> = snapshot
        .memory
        .days
        .iter()
        .map(|day| {
            usage
                .days
                .iter()
                .find(|recorded| recorded.date == day.date && recorded.total() > 0)
                .map_or_else(Vec::new, |recorded| {
                    vec![
                        (recorded.with_memory, series),
                        (recorded.without_memory, muted),
                    ]
                })
        })
        .collect();
    let slots = snapshot.memory.days.len();
    let plot = columns_chart(&columns, None, CHART_HEIGHT, cx);
    let hovered = screen
        .hovered_slot(Metric::AgentUsage, slots)
        .filter(|slot| columns[*slot].iter().any(|(value, _)| *value > 0));
    let tip = hovered.and_then(|slot| {
        let date = snapshot.memory.days[slot].date;
        usage
            .days
            .iter()
            .find(|recorded| recorded.date == date && recorded.total() > 0)
            .map(|recorded| {
                day_tip(
                    timestamps::day(date),
                    format!(
                        "{} with Memory · {} without",
                        thousands(recorded.with_memory),
                        thousands(recorded.without_memory)
                    ),
                    cx,
                )
            })
    });
    Content::drawn(
        None,
        with_hover(Metric::AgentUsage, slots, hovered, tip, plot, cx),
        period_labels(snapshot),
        vec![("Called Memory", series), ("No Memory call", muted)],
        cx,
    )
    .with_headline(headline.into_any_element())
}

impl Content {
    /// Puts a panel's headline above its chart, which is where macOS draws the
    /// one figure the chart is about.
    fn with_headline(mut self, headline: AnyElement) -> Self {
        self.element = div()
            .v_flex()
            .h_full()
            .justify_end()
            .gap_2()
            .child(headline)
            .child(self.element)
            .into_any_element();
        self
    }
}

/// The inventory as macOS draws it: `AreaMark` under `LineMark`, one point per
/// day the Server could count, in that day's place on the axis.
///
/// It is painted rather than handed to the component library's `AreaChart`,
/// which labels its x axis with the number it was given — here a Unix second.
/// The library's own plot shapes are painted this way too, and this is the same
/// two paths: a filled area closed on the baseline, and a stroked line through
/// the points.
///
/// A day the Server could not count is absent and the line steps over it, which
/// is what macOS's filtered points do.
fn inventory_chart(
    slots: usize,
    points: &[(usize, usize)],
    peak: usize,
    height: f32,
    series: Hsla,
    cx: &App,
) -> AnyElement {
    // macOS fills the same area with its own blue at a fifth: `blue.opacity(0.20)`
    // fading out, against a 2.5pt line. Its y domain leaves a fifth of headroom
    // above the peak — `0...(max * 12/10 + 1)` — so the line is never the top
    // edge of the panel.
    let area = series.opacity(0.18);
    let baseline = cx.theme().muted;
    let slots = slots.max(1);
    let peak = (peak.max(1) * 12 / 10 + 1) as f32;
    let points = points.to_vec();
    canvas(
        move |_bounds, _window, _cx| (),
        move |bounds, (), window, _cx| {
            let width = bounds.size.width.as_f32();
            let bottom = bounds.origin.y + bounds.size.height;
            let step = width / slots as f32;
            let x = |slot: usize| bounds.origin.x + px(step * (slot as f32 + 0.5));
            let y = |count: usize| bottom - px(height * count as f32 / peak);
            let first = points[0];
            let last = *points.last().unwrap_or(&first);

            let mut filled = PathBuilder::fill();
            filled.move_to(point(x(first.0), bottom));
            for (slot, count) in &points {
                filled.line_to(point(x(*slot), y(*count)));
            }
            filled.line_to(point(x(last.0), bottom));
            filled.close();
            if let Ok(path) = filled.build() {
                window.paint_path(path, area);
            }

            // One day is a point, as macOS draws `PointMark` for it; more than
            // one is the stroke through them.
            if points.len() == 1 {
                window.paint_quad(
                    fill(
                        Bounds::new(
                            point(x(first.0) - px(3.), y(first.1) - px(3.)),
                            size(px(6.), px(6.)),
                        ),
                        series,
                    )
                    .corner_radii(px(3.)),
                );
                return;
            }
            let mut stroke = PathBuilder::stroke(px(2.));
            stroke.move_to(point(x(first.0), y(first.1)));
            for (slot, count) in &points[1..] {
                stroke.line_to(point(x(*slot), y(*count)));
            }
            if let Ok(path) = stroke.build() {
                window.paint_path(path, series);
            }
        },
    )
    .h(px(height))
    .border_b_1()
    .border_color(baseline)
    .into_any_element()
}

/// A column chart. One column per day, drawn bottom-up from its segments.
///
/// `scale` is what a column's full height means: a count chart passes the
/// period's peak, so the shape of the period is readable against it. A share
/// chart passes nothing, and every column fills the height split by its own
/// proportions — which is what macOS draws on a 0...1 y-scale. There is no y
/// axis: each panel names its peak above the chart instead, which is what a
/// reader takes from a column at this size.
///
/// The columns are painted rather than composed: a month of three series is
/// ninety nested boxes that the layout engine and the rasteriser both walk
/// every frame, and the component library's own plot shapes paint their bars
/// this way too. A column is a day on the axis, not a slab, so a thin column
/// keeps the width it earns and a wide one is capped.
fn columns_chart(
    columns: &[Vec<(usize, Hsla)>],
    scale: Option<usize>,
    height: f32,
    cx: &App,
) -> AnyElement {
    let baseline = cx.theme().muted;
    let columns = columns.to_vec();
    canvas(
        move |_bounds, _window, _cx| (),
        move |bounds, (), window, _cx| {
            let width = bounds.size.width.as_f32();
            let bottom = bounds.origin.y + bounds.size.height;
            let step = width / columns.len().max(1) as f32;
            let gap = if step > 3. { 1. } else { 0. };
            let bar = (step - gap).clamp(1., 24.);
            for (slot, segments) in columns.iter().enumerate() {
                let total: usize = segments.iter().map(|(value, _)| *value).sum();
                let unit = match scale {
                    Some(peak) => peak.max(1) as f32,
                    None => total.max(1) as f32,
                };
                let drawn: Vec<(usize, Hsla)> = segments
                    .iter()
                    .copied()
                    .filter(|(value, _)| *value > 0)
                    .collect();
                let x = bounds.origin.x + px(step * slot as f32 + (step - bar) / 2.);
                let mut y = bottom;
                for (index, (value, color)) in drawn.iter().enumerate() {
                    let segment = (height * *value as f32 / unit).max(1.);
                    let top = y - px(segment);
                    // Only the ends of a column are rounded, which is what the
                    // stacked segments of a day add up to.
                    let radii = if index + 1 == drawn.len() {
                        Corners {
                            top_left: px(2.),
                            top_right: px(2.),
                            bottom_right: px(0.),
                            bottom_left: px(0.),
                        }
                    } else if index == 0 {
                        Corners {
                            top_left: px(0.),
                            top_right: px(0.),
                            bottom_right: px(2.),
                            bottom_left: px(2.),
                        }
                    } else {
                        Corners::default()
                    };
                    window.paint_quad(
                        fill(
                            Bounds::new(point(x, top), size(px(bar), px(segment))),
                            *color,
                        )
                        .corner_radii(radii),
                    );
                    y = top;
                }
            }
        },
    )
    .h(px(height))
    .border_b_1()
    .border_color(baseline)
    .into_any_element()
}

/// Three dates under a chart: where the period starts, its middle, and where it
/// ends. macOS draws tick marks instead; a column chart this size reads the same
/// with the ends named and nothing between them.
fn axis(labels: (String, String, String), cx: &App) -> AnyElement {
    div()
        .h_flex()
        .justify_between()
        .text_style(&ui::CAPTION)
        .text_color(cx.theme().muted_foreground)
        .child(labels.0)
        .child(labels.1)
        .child(labels.2)
        .into_any_element()
}

/// The first, middle and last date of a run of days.
fn labels(dates: &[i64]) -> (String, String, String) {
    let first = dates.first().copied().unwrap_or_default();
    let last = dates.last().copied().unwrap_or_default();
    let middle = dates.get(dates.len() / 2).copied().unwrap_or(first);
    (
        timestamps::day(first),
        timestamps::day(middle),
        timestamps::day(last),
    )
}

/// The key to a chart's colours, which macOS draws under the plot.
fn chart_legend(items: &[(&'static str, Hsla)], cx: &App) -> Option<AnyElement> {
    if items.is_empty() {
        return None;
    }
    Some(
        div()
            .h_flex()
            .items_center()
            .gap_3()
            .text_style(&ui::CAPTION)
            .text_color(cx.theme().muted_foreground)
            .children(items.iter().map(|(label, color)| legend_dot(label, *color)))
            .into_any_element(),
    )
}

/// Horizontal bars: a label, a length, and the count at the end. macOS's
/// `horizontalBars`, where a coverage bar draws the inventory behind the
/// retrieved count and a plain bar is the count itself.
fn bar_chart(
    rows: Vec<DashboardBar>,
    coverage: bool,
    open_in_memory: Option<&DashboardSnapshot>,
    cx: &mut Context<DesktopApp>,
) -> AnyElement {
    let (primary, muted, foreground, muted_foreground) = {
        let theme = cx.theme();
        (
            // The same blue the column charts draw in, for the same reason:
            // `primary` is the foreground in dark mode.
            theme.chart_3,
            theme.muted,
            theme.foreground,
            theme.muted_foreground,
        )
    };
    let peak = rows
        .iter()
        .map(|row| {
            if coverage {
                row.total.unwrap_or(row.value)
            } else {
                row.value
            }
        })
        .max()
        .unwrap_or(1)
        .max(1);

    let bars: Vec<AnyElement> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let track = if coverage {
                row.total.unwrap_or(row.value)
            } else {
                row.value
            };
            let value = if coverage {
                format!("{} / {}", thousands(row.value), thousands(track))
            } else {
                thousands(row.value)
            };
            // macOS makes the label of a retrieved document a button that opens
            // it in Memory; the row itself stays a reading surface.
            let label = match open_in_memory.and_then(|snapshot| snapshot.path_for(&row.id)) {
                Some(path) => {
                    let opening = path.to_owned();
                    div()
                        .id(("dashboard-bar", index))
                        .w(px(BAR_LABEL_WIDTH))
                        .flex_shrink_0()
                        .truncate()
                        .cursor_pointer()
                        .text_style(&ui::CAPTION)
                        .text_color(foreground)
                        .hover(|style| style.text_color(primary))
                        .tooltip({
                            let label = row.label.clone();
                            move |window, cx| Tooltip::new(label.clone()).build(window, cx)
                        })
                        .on_click(cx.listener(move |app, _event, window, cx| {
                            app.open_memory_resource(&opening, window, cx);
                        }))
                        .child(row.label.clone())
                        .into_any_element()
                }
                None => div()
                    .w(px(BAR_LABEL_WIDTH))
                    .flex_shrink_0()
                    .truncate()
                    .text_style(&ui::CAPTION)
                    .text_color(foreground)
                    .child(row.label.clone())
                    .into_any_element(),
            };
            let mut field = div()
                .relative()
                .flex_1()
                .min_w(px(0.))
                .h(px(16.))
                .rounded(px(3.));
            if coverage {
                field = field.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(relative(track as f32 / peak as f32))
                        .rounded(px(3.))
                        .bg(muted),
                );
            }
            field = field.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .h_full()
                    .w(relative(row.value as f32 / peak as f32))
                    .rounded(px(3.))
                    .bg(primary),
            );
            div()
                .h_flex()
                .items_center()
                .gap_3()
                .child(label)
                .child(field)
                .child(
                    div()
                        .w(px(if coverage { 76. } else { 56. }))
                        .flex_shrink_0()
                        .text_right()
                        .text_style(&ui::CAPTION)
                        .text_color(muted_foreground)
                        .child(value),
                )
                .into_any_element()
        })
        .collect();

    let mut column = div().v_flex().w_full().gap_3().children(bars);
    if coverage {
        column = column.child(
            div()
                .h_flex()
                .items_center()
                .gap_3()
                .child(legend_dot("Retrieved", primary))
                .child(legend_dot("Total", muted)),
        );
    }
    div()
        .v_flex()
        .h_full()
        .justify_end()
        .child(column)
        .into_any_element()
}

/// What a panel draws when it has nothing to draw, which is the same shape
/// macOS's `empty(_:detail:)` has.
fn empty_panel(title: &'static str, detail: &'static str, cx: &App) -> AnyElement {
    let (muted, foreground) = {
        let theme = cx.theme();
        (theme.muted_foreground, theme.foreground)
    };
    div()
        .v_flex()
        .flex_1()
        .h_full()
        .justify_center()
        .items_center()
        .gap_2()
        .child(
            Icon::default()
                .path("icons/chart-bar.svg")
                .with_size(px(24.))
                .text_color(muted),
        )
        .child(
            div()
                .text_style(&ui::BODY)
                .text_color(foreground)
                .child(title),
        )
        .child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(muted)
                .child(detail),
        )
        .into_any_element()
}

/// A coloured dot and its name, which is what a chart's legend is.
fn legend_dot(label: &str, color: Hsla) -> AnyElement {
    div()
        .h_flex()
        .items_center()
        .gap_2()
        .child(div().size(px(7.)).rounded_full().bg(color))
        .child(label.to_owned())
        .into_any_element()
}

/// The About sheet: what one panel counts, in macOS's own words.
pub struct AboutDialog {
    metric: Metric,
    /// How many requests per Project the engine keeps, when there is a snapshot
    /// to read it from.
    retention: Option<i64>,
}

impl AboutDialog {
    pub fn new(metric: Metric, retention: Option<i64>) -> Self {
        Self { metric, retention }
    }
}

impl Render for AboutDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (foreground, muted) = {
            let theme = cx.theme();
            (theme.foreground, theme.muted_foreground)
        };
        let mut column = div().v_flex().w_full().gap_4().child(
            div()
                .text_style(&ui::BODY)
                .text_color(foreground)
                .child(self.metric.explanation()),
        );
        if self.metric.explains_local_history()
            && let Some(limit) = self.retention
        {
            column = column.child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(muted)
                    .child(format!(
                        "Based on retrieval history retained on this machine, up to {limit} requests per project. Activity on other devices is not included."
                    )),
            );
        }
        column
    }
}

/// The badge that says the page is a sample: macOS's "Demo data" capsule,
/// which a Dev Instance's fixture is always shown with.
fn demo_badge(cx: &App) -> AnyElement {
    let (surface, tone) = {
        let theme = cx.theme();
        (theme.muted, theme.chart_3)
    };
    div()
        .h_flex()
        .self_start()
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(surface)
        .child(
            Icon::default()
                .path("icons/sparkles.svg")
                .with_size(px(12.))
                .text_color(tone),
        )
        .child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(tone)
                .child("Demo data"),
        )
        .into_any_element()
}

/// A count with its thousands separated, which is what macOS's `.formatted()`
/// gives a reader and what a bare run of six digits does not.
fn thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    grouped
}

/// A rate as the whole percent a reader compares, which is macOS's
/// `.percent.precision(.fractionLength(0))`.
fn percent(rate: f64) -> String {
    format!("{:.0}%", rate * 100.)
}

#[cfg(test)]
mod tests {
    // Only what the tests use: the component library exports a `test` macro of
    // its own, and a glob import would shadow the built-in attribute with it.
    use super::{directory_rows, labels, percent, thousands};
    use crate::engine::{DashboardBar, RetrievalStatistics};
    use crate::timestamps;

    #[test]
    fn counts_are_grouped_the_way_a_reader_reads_them() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(12_345), "12,345");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn a_rate_is_a_whole_percent() {
        assert_eq!(percent(0.), "0%");
        assert_eq!(percent(0.615), "62%");
        assert_eq!(percent(1.), "100%");
    }

    #[test]
    fn a_long_directory_list_folds_into_one_row() {
        fn rows(count: usize) -> Vec<DashboardBar> {
            (0..count)
                .map(|index| DashboardBar {
                    id: format!("dir_{index}"),
                    label: format!("dir{index}"),
                    value: index + 1,
                    total: Some((index + 1) * 2),
                })
                .collect()
        }
        fn retrieval(directories: Vec<DashboardBar>) -> RetrievalStatistics {
            RetrievalStatistics {
                retrievals: 0,
                recalled_count: 0,
                coverage: 0.,
                days: Vec::new(),
                directories,
                top_resources: Vec::new(),
                history_start: None,
                retention_per_project: 500,
                delta: None,
                agent_usage: None,
            }
        }
        // Six rows are drawn as they are; the seventh becomes "Other".
        assert_eq!(directory_rows(&retrieval(rows(6))).len(), 6);
        let folded = directory_rows(&retrieval(rows(8)));
        assert_eq!(folded.len(), 6);
        assert_eq!(folded[5].label, "Other directories");
        assert_eq!(folded[5].value, 6 + 7 + 8);
        assert_eq!(folded[5].total, Some(12 + 14 + 16));
    }

    #[test]
    fn a_chart_names_the_days_it_covers() {
        let days = [1_790_452_800, 1_790_539_200, 1_790_625_600];
        let (first, middle, last) = labels(&days);
        assert_eq!(first, timestamps::day(days[0]));
        assert_eq!(middle, timestamps::day(days[1]));
        assert_eq!(last, timestamps::day(days[2]));
    }
}
