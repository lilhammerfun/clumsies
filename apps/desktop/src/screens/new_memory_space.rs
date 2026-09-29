//! A dialog for making a memory space: the Project a reader then works in.
//!
//! The Server owns the record, so this asks for the two things it takes — a
//! name, and what the space says about itself — and the window moves into the
//! space the Server answers with. macOS creates a Project the same way, from its
//! own project-management sheet; here it is the entry the Project filter was
//! missing, which is the one thing that stopped a new account from starting.

use gpui_kit::base::StyledExt;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme, WindowExt as _};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::modal;
use crate::engine;
use crate::ui::{self, Typography};

pub struct NewMemorySpaceDialog {
    app: WeakEntity<DesktopApp>,
    name: Entity<InputState>,
    description: Entity<InputState>,
    busy: bool,
    error: Option<String>,
}

impl NewMemorySpaceDialog {
    pub fn open(app: WeakEntity<DesktopApp>, window: &mut Window, cx: &mut App) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Payments platform"));
        let description =
            cx.new(|cx| InputState::new(window, cx).placeholder("What this memory space is for"));
        let view = cx.new(|_| Self {
            app,
            name,
            description,
            busy: false,
            error: None,
        });
        let footer_view = view.clone();
        modal::open(
            window,
            cx,
            "New memory space",
            modal::NARROW,
            move |dialog, _window, cx| {
                let ready = footer_view.read(cx).ready(cx);
                let create = modal::primary("new-memory-space-create", "Create", ready)
                    .on_click({
                        let view = footer_view.clone();
                        move |_event, window, cx| {
                            let _ = view.update(cx, |dialog, cx| dialog.create(window, cx));
                        }
                    })
                    .into_any_element();
                dialog
                    .content({
                        let view = view.clone();
                        move |content, _window, _cx| content.child(view.clone())
                    })
                    .footer(modal::footer(Some(modal::cancel("Cancel", true)), create))
            },
        );
    }

    /// A name is what a memory space cannot do without; the description is how
    /// its members tell two of them apart.
    fn ready(&self, cx: &App) -> bool {
        !self.busy && !self.name.read(cx).value().trim().is_empty()
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        let name = self.name.read(cx).value().trim().to_owned();
        let description = self.description.read(cx).value().trim().to_owned();
        self.busy = true;
        self.error = None;
        cx.notify();
        let app = self.app.clone();
        let task = cx.background_executor().spawn(async move {
            engine::create_project(&name, &description).map(|project| project.project_id)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |dialog, window, cx| {
                dialog.busy = false;
                match result {
                    Ok(project_id) => {
                        // The window moves into the space the Server made, and
                        // this dialog is done.
                        let _ = app.update(cx, |app, cx| {
                            app.memory_space_created(&project_id, cx);
                        });
                        window.close_dialog(cx);
                    }
                    Err(error) => {
                        dialog.error = Some(error);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Render for NewMemorySpaceDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child("A memory space is this team's Memory: the documents its agents read, versioned together and reviewed before they merge. You can invite members and connect repositories once it exists."),
            )
            .child(Input::new(&self.name))
            .child(Input::new(&self.description))
            .children(self.error.as_ref().map(|error| {
                div()
                    .text_style(&ui::BODY)
                    .text_color(cx.theme().danger)
                    .child(error.clone())
            }))
    }
}
