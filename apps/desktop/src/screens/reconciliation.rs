//! A domain dialog composed from the shared modal, GPUI inputs/buttons and diff.
use crate::{
    app::DesktopApp,
    components::{diff, modal},
    engine::{self, ReconciliationCandidate, ReconciliationState},
    ui,
};
use gpui_kit::base::StyledExt;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme, WindowExt};
use gpui_kit::*;

pub struct ReconciliationDialog {
    app: WeakEntity<DesktopApp>,
    project: String,
    candidate: Option<ReconciliationCandidate>,
    error: Option<String>,
    selected: Option<ReconciliationState>,
    path: Entity<InputState>,
    content: Entity<TextareaState>,
}
impl ReconciliationDialog {
    pub fn open(
        app: WeakEntity<DesktopApp>,
        project: String,
        draft: String,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
    ) {
        let path = cx.new(|cx| InputState::new(window, cx));
        let content = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(true));
        let view = cx.new(|_| Self {
            app,
            project,
            candidate: None,
            error: None,
            selected: None,
            path,
            content,
        });
        view.update(cx, |_view, cx| {
            let work = cx
                .background_executor()
                .spawn(async move { engine::reconciliation_candidate(&draft) });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                this.update(cx, |view, cx| {
                    match result {
                        Ok(candidate) => view.candidate = Some(candidate),
                        Err(error) => view.error = Some(error),
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        });
        let footer = view.clone();
        modal::open(
            window,
            cx,
            "Update draft",
            modal::WIDE,
            move |dialog, _, cx| {
                let state = footer.read(cx);
                let ready = state.candidate.as_ref().is_some_and(|c| c.valid)
                    && (state.selected.is_some()
                        || state
                            .candidate
                            .as_ref()
                            .is_some_and(|c| c.conflicts.is_empty()));
                let view = footer.clone();
                let apply = modal::primary("apply-reconciliation", "Apply", ready)
                    .on_click(move |_, window, cx| {
                        view.update(cx, |view, cx| view.apply(window, cx));
                    })
                    .into_any_element();
                let content = footer.clone();
                dialog
                    .content(move |body, _, _| body.child(content.clone()))
                    .footer(modal::footer(Some(modal::cancel("Cancel", true)), apply))
            },
        );
    }
    fn choose(&mut self, state: ReconciliationState, window: &mut Window, cx: &mut Context<Self>) {
        let path = state.resource["path"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let content = state
            .content
            .as_ref()
            .map(|c| c.content.clone())
            .unwrap_or_default();
        self.path
            .update(cx, |input, cx| input.set_value(path, window, cx));
        self.content
            .update(cx, |input, cx| input.set_value(content, window, cx));
        self.selected = Some(state);
        cx.notify();
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(candidate) = self.candidate.clone() else {
            return;
        };
        let mut resolved = if candidate.conflicts.is_empty() {
            None
        } else {
            self.selected.clone()
        };
        if let Some(state) = resolved.as_mut() {
            let path = self.path.read(cx).value().to_string();
            if state.exists && !crate::memory_paths::valid(&path) {
                self.error = Some("Enter a valid relative path".into());
                cx.notify();
                return;
            }
            state.resource["path"] = path.into();
            if let Some(content) = state.content.as_mut() {
                if !content.is_directory {
                    content.content = self.content.read(cx).value().to_string();
                }
            }
        } else if !candidate.conflicts.is_empty() {
            return;
        }
        let project = self.project.clone();
        self.app
            .update(cx, |app, cx| {
                app.apply_memory_reconciliation(project, candidate, resolved, cx)
            })
            .ok();
        window.close_dialog(cx);
    }
}
impl Render for ReconciliationDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().v_flex().gap_3();
        if let Some(error) = &self.error {
            body = body.child(ui::message(error.clone(), cx.theme().danger));
        }
        let Some(candidate) = self.candidate.as_ref() else {
            return body.child("Loading current and draft versions…");
        };
        body=body.child(if candidate.conflicts.is_empty(){"The server can update this draft automatically."}else{"Choose a version as the starting point, then edit the path or content if needed. Applying changes the draft, not published Memory."});
        for conflict in &candidate.conflicts {
            body = body.child(ui::message(
                format!(
                    "{}: {}\nBase: {}\nCurrent: {}\nDraft: {}",
                    conflict.field,
                    conflict.kind,
                    conflict.base.as_deref().unwrap_or("—"),
                    conflict.current.as_deref().unwrap_or("—"),
                    conflict.draft.as_deref().unwrap_or("—")
                ),
                cx.theme().muted_foreground,
            ));
        }
        let current = candidate.current_state.clone();
        let draft = candidate
            .proposed_state
            .as_ref()
            .unwrap_or(&candidate.draft_state)
            .clone();
        let before = current
            .content
            .as_ref()
            .map(|c| c.content.as_str())
            .unwrap_or("");
        let after = draft
            .content
            .as_ref()
            .map(|c| c.content.as_str())
            .unwrap_or("");
        body = body.child(div().h(px(180.)).child(diff::diff_view(
            diff::diff_rows(before, after),
            cx.theme().mono_font_family.clone(),
            diff::DiffPalette::from_theme(cx.theme()),
            window,
        )));
        let current_label = if current.exists {
            "Use current version"
        } else {
            "Use current deletion"
        };
        let draft_label = if draft.exists {
            "Use draft version"
        } else {
            "Use draft deletion"
        };
        if !candidate.conflicts.is_empty() {
            body =
                body.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(Button::new("use-current").label(current_label).on_click(
                            cx.listener(move |view, _, window, cx| {
                                view.choose(current.clone(), window, cx)
                            }),
                        ))
                        .child(
                            Button::new("use-draft")
                                .label(draft_label)
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.choose(draft.clone(), window, cx)
                                })),
                        ),
                );
        }
        if let Some(state) = &self.selected {
            if state.exists {
                body = body.child(Input::new(&self.path));
                if state.content.as_ref().is_some_and(|c| !c.is_directory) {
                    body = body.child(Textarea::new(&self.content).h(px(160.)));
                }
            } else {
                body = body.child("The resolved draft deletes this resource.");
            }
        }
        body
    }
}
