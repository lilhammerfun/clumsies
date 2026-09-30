//! The dialog that starts a new Memory document.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::modal;
use crate::ui::{self, Typography};

pub struct NewMemoryDialog {
    app: WeakEntity<DesktopApp>,
    /// The folder the new document goes in, which is the row it was asked for
    /// from; empty means the Project's root.
    folder: String,
    directory: bool,
    name: Entity<InputState>,
}

impl NewMemoryDialog {
    pub fn open(
        app: WeakEntity<DesktopApp>,
        folder: &str,
        suggested: &str,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
    ) {
        Self::open_kind(app, folder, suggested, false, window, cx);
    }

    pub fn open_kind(
        app: WeakEntity<DesktopApp>,
        folder: &str,
        suggested: &str,
        directory: bool,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
    ) {
        let field = cx.new(|cx| InputState::new(window, cx).default_value(suggested.to_owned()));
        let folder = folder.to_owned();
        let view = cx.new(|_| Self {
            app,
            folder,
            name: field,
            directory,
        });
        let footer_view = view.clone();
        modal::open(
            window,
            cx,
            if directory {
                "New folder"
            } else {
                "New memory"
            },
            modal::NARROW,
            move |dialog, _window, cx| {
                let ready = footer_view.read(cx).ready(cx);
                let create = modal::primary("new-memory-create", "Create", ready)
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

    /// A file name, not a path: the folder is the row this was asked for from,
    /// and the daemon validates the path it is given.
    fn ready(&self, cx: &App) -> bool {
        let name = self.name.read(cx).value().trim().to_owned();
        crate::memory_paths::valid(&name) && !name.contains('/')
    }

    fn create(&mut self, window: &mut Window, cx: &mut App) {
        if !self.ready(cx) {
            return;
        }
        let name = self.name.read(cx).value().trim().to_owned();
        let path = if self.folder.is_empty() {
            name
        } else {
            format!("{}/{name}", self.folder)
        };
        let _ = self.app.update(cx, |app, cx| {
            app.create_memory_entry(&path, self.directory, cx)
        });
        window.close_dialog(cx);
    }
}

impl Render for NewMemoryDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = if self.folder.is_empty() {
            "this Project".to_owned()
        } else {
            self.folder.clone()
        };
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "A new {} in {shown}. It is saved as a draft and shared after review and merge.", if self.directory { "folder" } else { "document" }
                    )),
            )
            .child(Input::new(&self.name))
    }
}
