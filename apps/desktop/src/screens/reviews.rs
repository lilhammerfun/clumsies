//! The Reviews screen: what a Project's drafts propose, and the decision each
//! one waits for.
//!
//! Read from the macOS client's ReviewListPage and ReviewDetailPage. The list is
//! a queue in the Server's own order — newest update first — and the detail is
//! the Review's header, its changed documents, and a unified diff for the one
//! being read. The actions macOS keeps in its window toolbar are in this
//! client's pane header, because every command in this window lives beside the
//! region it acts on.
//!
//! Not built yet, and named here rather than left to be discovered: the status
//! filter and the search field, the comments (review-wide and inline),
//! resubmission, the reconciliation surface a behind Review needs, and the
//! permission checks macOS makes from its own capabilities — this client offers
//! the action and reports what the Server answers.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::diff;
use crate::engine::{
    Checkout, MemoryDocument, Review, ReviewDetail, ReviewStatus, ReviewedDocument,
};
use crate::ui::{self, Typography};

/// The width of the column listing a Review's changed documents.
const NAVIGATOR_WIDTH: f32 = 220.;

/// What a Review's state looks like at a glance: macOS's
/// ReviewQueueStatePresentation, with the same order of precedence.
struct State {
    title: &'static str,
    icon: &'static str,
    tone: Tone,
    /// Whether this is something a reader has to act on. A state that is only
    /// news is drawn as the icon alone; one that is waiting on someone is a
    /// badge beside the title.
    signal: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Neutral,
    Positive,
    /// Published: the end of the road, which macOS draws in its accent.
    Done,
    Negative,
}

impl State {
    /// The state of one Review. macOS asks for the reader's identity and
    /// permissions here as well, because it has them; this client has neither
    /// modelled yet, so a rejected Review reads as "Awaiting Author" for
    /// everyone rather than guessing at who is looking at it.
    fn of(review: &Review) -> Self {
        if review.status == ReviewStatus::Merged {
            return Self {
                title: "Merged",
                icon: "icons/git-merge.svg",
                tone: Tone::Done,
                signal: false,
            };
        }
        if review.coordination.has_conflicts() {
            return Self {
                title: "Conflict",
                icon: "icons/triangle-alert.svg",
                tone: Tone::Negative,
                signal: true,
            };
        }
        match review.status {
            ReviewStatus::Open => Self {
                title: "Needs Review",
                icon: "icons/clock.svg",
                tone: Tone::Neutral,
                signal: false,
            },
            ReviewStatus::Approved if review.can_merge() => Self {
                title: "Ready to Merge",
                icon: "icons/git-merge.svg",
                tone: Tone::Positive,
                signal: true,
            },
            ReviewStatus::Approved => Self {
                title: "Approved",
                icon: "icons/circle-check.svg",
                tone: Tone::Positive,
                signal: false,
            },
            ReviewStatus::Rejected => Self {
                title: "Awaiting Author",
                icon: "icons/clock.svg",
                tone: Tone::Neutral,
                signal: true,
            },
            ReviewStatus::Merged => unreachable!("handled above"),
        }
    }
}

fn tone_color(tone: Tone, cx: &App) -> Hsla {
    let theme = cx.theme();
    match tone {
        Tone::Neutral => theme.muted_foreground,
        Tone::Positive => theme.success,
        Tone::Done => theme.primary,
        Tone::Negative => theme.danger,
    }
}

/// Something the screen has to say about what it just did.
pub struct ReviewNotice {
    pub text: String,
    pub failed: bool,
}

pub struct ReviewsScreen {
    /// The Project whose Reviews are listed. Another Project is another queue.
    project_id: Option<String>,
    reviews: Vec<Review>,
    /// The Review the reader has open, by id, so that a re-read of the list
    /// does not move them.
    open: Option<String>,
    detail: Option<ReviewDetail>,
    /// The Project's documents as the Server publishes them, which is what a
    /// proposal is measured against, and where a proposal that names only a
    /// resource gets its path.
    published: Vec<MemoryDocument>,
    /// Which of the open Review's documents is being read.
    document: Option<usize>,
    /// Why the queue or the open Review could not be read.
    list_error: Option<String>,
    detail_error: Option<String>,
    loading_list: bool,
    loading_detail: bool,
    /// What the last decision or merge answered, until the reader acts again.
    notice: Option<ReviewNotice>,
    /// Where the queue takes the keyboard, which is one of the regions F6
    /// walks.
    list_focus: FocusHandle,
}

impl ReviewsScreen {
    pub fn new(cx: &mut Context<DesktopApp>) -> Self {
        Self {
            project_id: None,
            reviews: Vec::new(),
            open: None,
            detail: None,
            published: Vec::new(),
            document: None,
            list_error: None,
            detail_error: None,
            loading_list: false,
            loading_detail: false,
            notice: None,
            list_focus: cx.focus_handle(),
        }
    }

    /// The Project whose Reviews this is, which is the one the rest of the
    /// window is looking at. Another Project empties the queue rather than
    /// showing one Project's Reviews under another's name.
    pub fn set_project(&mut self, project_id: Option<String>, cx: &mut Context<DesktopApp>) {
        if self.project_id == project_id {
            return;
        }
        self.project_id = project_id;
        self.reviews.clear();
        self.open = None;
        self.detail = None;
        self.document = None;
        self.list_error = None;
        self.detail_error = None;
        self.notice = None;
        self.published.clear();
        cx.notify();
    }

    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    /// The text the Project publishes today. A proposal is diffed against it,
    /// which is what a reviewer is deciding on.
    pub fn set_published(&mut self, checkout: &Checkout) {
        self.published = checkout.documents.clone();
    }

    /// The Project's document a proposal changes: the resource it names, or
    /// the path when it names none.
    fn published_document(&self, document: &ReviewedDocument) -> Option<&MemoryDocument> {
        match &document.resource_id {
            Some(id) => self
                .published
                .iter()
                .find(|published| &published.resource_id == id),
            None => self.published.iter().find(|published| {
                document
                    .path
                    .as_deref()
                    .is_some_and(|path| published.path == path)
            }),
        }
    }

    /// What a proposal would publish, what it would publish it over, and what
    /// to call it: the three things the content pane needs.
    fn proposed_change(&self, document: &ReviewedDocument) -> (String, String, String) {
        let published = self.published_document(document);
        let path = document
            .path
            .clone()
            .or_else(|| published.map(|published| published.path.clone()))
            .unwrap_or_else(|| "a document without a path".to_owned());
        let base = published
            .map(|published| published.content.clone())
            .unwrap_or_default();
        let proposed = document.content.clone().unwrap_or_default();
        (path, base, proposed)
    }

    pub fn begin_list_read(&mut self) {
        self.loading_list = true;
        self.list_error = None;
    }

    /// Whether the queue has to be read before it can be shown: it has never
    /// been read, or the last read failed. A queue that answered stays as it is
    /// until something changes it.
    pub fn needs_reading(&self) -> bool {
        !self.loading_list && self.reviews.is_empty() && self.list_error.is_none()
    }

    pub fn set_list_error(&mut self, error: String, cx: &mut Context<DesktopApp>) {
        self.loading_list = false;
        self.list_error = Some(error);
        cx.notify();
    }

    pub fn set_detail_error(&mut self, error: String, cx: &mut Context<DesktopApp>) {
        self.loading_detail = false;
        self.detail_error = Some(error);
        self.detail = None;
        cx.notify();
    }

    /// Reads one of the open Review's documents, which is what the navigator is
    /// for.
    pub fn show_document(&mut self, index: usize, cx: &mut Context<DesktopApp>) {
        self.document = Some(index);
        cx.notify();
    }

    /// The queue as the Server answered it, with the open Review still open if
    /// it is still there.
    pub fn set_reviews(
        &mut self,
        reviews: Vec<Review>,
        error: Option<String>,
        cx: &mut Context<DesktopApp>,
    ) {
        self.loading_list = false;
        self.list_error = error;
        self.reviews = reviews;
        if self
            .open
            .as_ref()
            .is_some_and(|id| !self.reviews.iter().any(|review| &review.review_id == id))
        {
            self.open = None;
            self.detail = None;
            self.document = None;
        }
        cx.notify();
    }

    pub fn open_id(&self) -> Option<&str> {
        self.open.as_deref()
    }

    pub fn begin_detail_read(&mut self, review_id: String) {
        self.open = Some(review_id);
        self.detail = None;
        self.document = None;
        self.detail_error = None;
        self.notice = None;
        self.loading_detail = true;
    }

    pub fn set_detail(
        &mut self,
        detail: ReviewDetail,
        error: Option<String>,
        cx: &mut Context<DesktopApp>,
    ) {
        self.loading_detail = false;
        self.detail_error = error;
        self.document = (!detail.documents.is_empty()).then_some(0);
        self.detail = Some(detail);
        // The list carries the same Review, and a decision changes it, so the
        // queue is re-read whenever its detail arrives.
        cx.notify();
    }

    /// The Review the reader has open, as the queue knows it.
    pub fn open_review(&self) -> Option<&Review> {
        let id = self.open.as_deref()?;
        self.reviews.iter().find(|review| review.review_id == id)
    }

    pub fn set_notice(&mut self, notice: Option<ReviewNotice>, cx: &mut Context<DesktopApp>) {
        self.notice = notice;
        cx.notify();
    }

    pub fn can_decide(&self) -> bool {
        self.open_review().is_some_and(Review::can_decide)
    }

    pub fn can_approve(&self) -> bool {
        self.open_review().is_some_and(Review::can_approve)
    }

    pub fn can_merge(&self) -> bool {
        self.open_review().is_some_and(Review::can_merge)
    }

    /// The id the keyboard's next move lands on, without opening it: the app
    /// owns the read that follows.
    pub fn selection_after(&self, step: isize) -> Option<String> {
        if self.reviews.is_empty() {
            return None;
        }
        let current = self.open.as_ref().and_then(|id| {
            self.reviews
                .iter()
                .position(|review| &review.review_id == id)
        });
        let index = match current {
            Some(index) => {
                (index as isize + step).clamp(0, self.reviews.len() as isize - 1) as usize
            }
            None => {
                if step < 0 {
                    self.reviews.len() - 1
                } else {
                    0
                }
            }
        };
        let id = &self.reviews[index].review_id;
        (self.open.as_deref() != Some(id.as_str())).then(|| id.clone())
    }

    pub fn focus_list(&mut self, window: &mut Window, cx: &mut App) {
        window.focus(&self.list_focus, cx);
    }

    pub fn list_focused(&self, window: &Window) -> bool {
        self.list_focus.is_focused(window)
    }

    /// The section's list column: the queue of Reviews, under a header row of
    /// its own that names the section.
    pub fn list(
        &self,
        project: AnyElement,
        window: &Window,
        cx: &mut Context<DesktopApp>,
    ) -> AnyElement {
        let header = div()
            .h_flex()
            .h(px(crate::screens::document::PANE_HEADER))
            .px_3()
            .gap_2()
            .items_center()
            .child(div().text_style(&ui::BODY).child("Reviews"))
            .child(project);
        let ring = if self.list_focused(window) {
            cx.theme().ring
        } else {
            transparent_black()
        };

        let mut rows: Vec<AnyElement> = Vec::with_capacity(self.reviews.len());
        for (index, review) in self.reviews.iter().enumerate() {
            rows.push(self.row(index, review, cx));
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .p_2()
                    .child(ui::message(
                        self.queue_message(),
                        cx.theme().muted_foreground,
                    ))
                    .into_any_element(),
            );
        }

        div()
            .v_flex()
            .h_full()
            .child(header)
            .child(ui::rule(cx))
            .child(
                div()
                    .id("reviews-queue")
                    .v_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .m_2()
                    .p_1()
                    .rounded(px(ui::RADIUS))
                    .border_1()
                    .border_color(ring)
                    .track_focus(&self.list_focus)
                    .tab_stop(true)
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    /// What an empty queue says, which depends on why it is empty.
    fn queue_message(&self) -> String {
        if let Some(error) = &self.list_error {
            return error.clone();
        }
        if self.loading_list {
            return "Loading Reviews…".to_owned();
        }
        if self.project_id.is_none() {
            return "Choose a Project to see its Reviews.".to_owned();
        }
        "No Reviews. Reviews created from synchronized drafts appear here.".to_owned()
    }

    /// One Review in the queue: its state, what it is called, who asked, and
    /// when it last changed. macOS's ReviewRow, without the row chrome the
    /// platform draws for it.
    fn row(&self, index: usize, review: &Review, cx: &mut Context<DesktopApp>) -> AnyElement {
        let state = State::of(review);
        let tone = tone_color(state.tone, cx);
        let selected = self.open.as_deref() == Some(review.review_id.as_str());
        let opening = review.review_id.clone();
        let theme = cx.theme();
        let (foreground, muted, hover, active) = (
            theme.foreground,
            theme.muted_foreground,
            theme.list_hover,
            theme.list_active,
        );
        let author = review.author.name().to_owned();
        let updated = stamp(&review.updated_at);
        let badge_text = state.signal.then_some(state.title);

        let mut row = div()
            .id(("review-row", index))
            .h_flex()
            .items_start()
            .gap_2()
            .px_2()
            .py_2()
            .rounded(px(ui::RADIUS))
            .cursor_pointer()
            .on_click(cx.listener(move |app, _event, _window, cx| app.open_review(&opening, cx)));
        row = if selected {
            row.bg(active)
        } else {
            row.hover(move |style| style.bg(hover))
        };

        row.child(
            div().flex_shrink_0().pt(px(2.)).child(
                Icon::default()
                    .path(state.icon)
                    .with_size(px(14.))
                    .text_color(tone),
            ),
        )
        .child(
            div()
                .v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap_1()
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
                                .text_style(&ui::BODY)
                                .text_color(foreground)
                                .child(review.title.clone()),
                        )
                        .children(badge_text.map(|text| state_badge(text, state.tone, cx))),
                )
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .text_style(&ui::CAPTION)
                        .text_color(muted)
                        .child(div().flex_1().min_w(px(0.)).truncate().child(author))
                        .child(div().flex_shrink_0().child(updated)),
                ),
        )
        .into_any_element()
    }

    /// The section's detail: the open Review, or what stands in its place.
    pub fn detail(&self, actions: Option<AnyElement>, cx: &mut Context<DesktopApp>) -> AnyElement {
        let Some(detail) = &self.detail else {
            let reason = if let Some(error) = &self.detail_error {
                ui::message(error.clone(), cx.theme().danger)
            } else if self.loading_detail {
                ui::message("Loading Review…", cx.theme().muted_foreground)
            } else if self.open.is_some() {
                ui::message(
                    "This Review is no longer in the workspace.",
                    cx.theme().muted_foreground,
                )
            } else {
                ui::message("Select a Review.", cx.theme().muted_foreground)
            };
            return div()
                .v_flex()
                .flex_1()
                .h_full()
                .min_w(px(0.))
                .p_4()
                .child(reason)
                .into_any_element();
        };

        let review = &detail.review;
        let state = State::of(review);
        let theme = cx.theme();
        let (foreground, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);

        let mut header = div()
            .v_flex()
            .flex_shrink_0()
            .gap_2()
            .px_4()
            .py_3()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_style(&ui::SUBTITLE)
                            .text_color(foreground)
                            .child(review.title.clone()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_1()
                            .text_style(&ui::CAPTION)
                            .text_color(muted)
                            .child(
                                Icon::default()
                                    .path(state.icon)
                                    .with_size(px(14.))
                                    .text_color(tone_color(state.tone, cx)),
                            )
                            .child(state.title),
                    )
                    .children(actions),
            )
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(muted)
                    .child(metadata(review)),
            );
        let description = review.description.trim();
        if !description.is_empty() {
            header = header.child(
                div()
                    .text_style(&ui::BODY)
                    .text_color(foreground)
                    .child(description.to_owned()),
            );
        }
        if let Some(decision) = decision_line(review, cx) {
            header = header.child(decision);
        }
        header = header.child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(muted)
                .child(scope_label(review)),
        );
        if review.coordination.has_conflicts() {
            header = header.child(ui::message(
                "This proposal's base has moved on, and the latest remote version conflicts with it. The author resolves that before this Review can be published.",
                muted,
            ));
        }
        if let Some(notice) = &self.notice {
            header = header.child(ui::message(
                notice.text.clone(),
                if notice.failed {
                    theme.danger
                } else {
                    theme.success
                },
            ));
        }

        div()
            .v_flex()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(header)
            .child(ui::rule(cx))
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.navigator(cx))
                    .child(div().w(px(1.)).flex_shrink_0().bg(border))
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .child(self.content(cx)),
                    ),
            )
            .into_any_element()
    }

    /// The open Review's changed documents, which is what the content beside
    /// this column is showing one of. macOS builds the same list out of paths.
    fn navigator(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let Some(detail) = &self.detail else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (foreground, muted, hover, active) = (
            theme.foreground,
            theme.muted_foreground,
            theme.list_hover,
            theme.list_active,
        );
        let mut rows: Vec<AnyElement> = Vec::with_capacity(detail.documents.len());
        for (index, document) in detail.documents.iter().enumerate() {
            let selected = self.document == Some(index);
            let mut row =
                div()
                    .id(("review-document", index))
                    .v_flex()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded(px(ui::RADIUS))
                    .cursor_pointer()
                    .on_click(cx.listener(move |app, _event, _window, cx| {
                        app.show_review_document(index, cx)
                    }))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::default()
                                    .path("icons/file-text.svg")
                                    .with_size(px(12.))
                                    .flex_shrink_0()
                                    .text_color(muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_style(&ui::CAPTION)
                                    .text_color(foreground)
                                    .child(name_of(&self.proposed_change(document).0)),
                            ),
                    )
                    .child(
                        div()
                            .text_style(&ui::CAPTION)
                            .text_color(muted)
                            .child(document.action_label()),
                    );
            row = if selected {
                row.bg(active)
            } else {
                row.hover(move |style| style.bg(hover))
            };
            rows.push(row.into_any_element());
        }
        let empty = rows.is_empty().then(|| {
            div()
                .px_2()
                .child(ui::message("No documents.", muted))
                .into_any_element()
        });

        div()
            .id("review-documents")
            .v_flex()
            .w(px(NAVIGATOR_WIDTH))
            .h_full()
            .flex_shrink_0()
            .gap_1()
            .p_2()
            .overflow_y_scroll()
            .children(rows)
            .children(empty)
            .into_any_element()
    }

    /// The text of the document being read: the proposal against what the
    /// Project publishes today.
    fn content(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let Some(detail) = &self.detail else {
            return div().into_any_element();
        };
        let Some(document) = self.document.and_then(|index| detail.documents.get(index)) else {
            return self.centred(
                "Select a document. Choose one of the Review's changed documents.",
                cx.theme().muted_foreground,
            );
        };
        if document.content.is_none() {
            return self.centred(
                "This Review deletes this document. There is no proposed text to render.",
                cx.theme().muted_foreground,
            );
        }
        let (_, base, proposed) = self.proposed_change(document);
        let rows = diff::diff_rows(&base, &proposed);
        if rows.iter().all(|row| row.kind == diff::DiffKind::Context) {
            return self.centred("No content changes.", cx.theme().muted_foreground);
        }
        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.))
            .child(diff::diff_view(
                rows,
                cx.theme().mono_font_family.clone(),
                diff::DiffPalette::from_theme(cx.theme()),
            ))
            .into_any_element()
    }

    fn centred(&self, text: &str, color: Hsla) -> AnyElement {
        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.))
            .p_4()
            .child(ui::message(text.to_owned(), color))
            .into_any_element()
    }

    /// Whether the open Review can be sent back, which is the one decision that
    /// is not also a publication. Approving is merging — one transaction on the
    /// Server — so the other command asks whether the Review can merge.
    pub fn can_reject(&self) -> bool {
        self.can_decide()
    }
}

/// What a row calls a document: the last segment of its path.
fn name_of(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

/// A small capsule that says what a row is waiting on.
fn state_badge(text: &'static str, tone: Tone, cx: &App) -> AnyElement {
    let color = tone_color(tone, cx);
    div()
        .flex_shrink_0()
        .px_2()
        .rounded(px(ui::RADIUS))
        .bg(color.opacity(0.15))
        .text_style(&ui::CAPTION)
        .text_color(color)
        .child(text)
        .into_any_element()
}

/// "<author> · <project>[ · Updated <time>]", which is macOS's metadata line
/// with the Project left to the caller to know.
fn metadata(review: &Review) -> String {
    format!(
        "{} · Updated {}",
        review.author.name(),
        stamp(&review.updated_at)
    )
}

fn scope_label(review: &Review) -> &'static str {
    if review.scope == crate::engine::Scope::Project {
        "Publishes to Project Memory"
    } else {
        "Publishes to Organization Memory"
    }
}

/// What the last decision said, for a Review that has one.
fn decision_line(review: &Review, cx: &App) -> Option<AnyElement> {
    if review.status == ReviewStatus::Open {
        return None;
    }
    let (title, color) = match review.status {
        ReviewStatus::Approved => ("Approved", cx.theme().success),
        ReviewStatus::Rejected => ("Changes requested", cx.theme().danger),
        ReviewStatus::Merged => ("Merged", cx.theme().primary),
        ReviewStatus::Open => return None,
    };
    let theme = cx.theme();
    let decider = review
        .decided_by
        .as_ref()
        .map(|user| user.name().to_owned());
    let decided_at = review.decided_at.as_deref().map(stamp);
    let mut line = div()
        .h_flex()
        .items_center()
        .gap_2()
        .child(div().text_style(&ui::BODY).text_color(color).child(title));
    if let Some(decider) = decider {
        line = line.child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(theme.muted_foreground)
                .child(format!("Decision by {decider}")),
        );
    }
    if let Some(decided_at) = decided_at {
        line = line.child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(theme.muted_foreground)
                .child(decided_at),
        );
    }
    let body = review
        .decision_body
        .as_deref()
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .map(|body| {
            div()
                .text_style(&ui::BODY)
                .text_color(theme.foreground)
                .child(body.to_owned())
                .into_any_element()
        });
    Some(
        div()
            .v_flex()
            .gap_2()
            .child(line)
            .children(body)
            .into_any_element(),
    )
}

/// The Server writes time as RFC 3339. Until a formatter lands, the date and
/// minute of that stamp is what a row shows: it is honest about the instant and
/// makes no claim about how long ago it was.
fn stamp(value: &str) -> String {
    let mut text = value.chars().take(16).collect::<String>();
    if let Some(index) = text.find('T') {
        text.replace_range(index..=index, " ");
    }
    text
}
