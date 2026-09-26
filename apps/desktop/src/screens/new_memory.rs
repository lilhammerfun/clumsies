//! The dialog that starts a new Memory document.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::ui::{self, Typography};

pub struct NewMemoryDialog {
    app: WeakEntity<DesktopApp>,
    /// The folder the new document goes in, which is the row it was asked for
    /// from; empty means the Project's root.
    folder: String,
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
        let field = cx.new(|cx| InputState::new(window, cx).default_value(suggested.to_owned()));
        let folder = folder.to_owned();
        let view = cx.new(|_| Self {
            app,
            folder,
            name: field,
        });
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let view = view.clone();
            dialog
                .title("New memory")
                .w(px(460.))
                .keyboard(true)
                .content(move |content, _window, _cx| content.child(view.clone()))
                .footer(div())
                .footer(div())
        });
    }
}

impl Render for NewMemoryDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = self.name.read(cx).value().trim().to_owned();
        // A file name, not a path: the folder is the row this was asked for
        // from, and the daemon validates the path it is given.
        let valid = !name.is_empty() && name != "." && name != ".." && !name.contains('/');
        let path = if self.folder.is_empty() {
            name.clone()
        } else {
            format!("{}/{name}", self.folder)
        };
        let confirming = self.app.clone();
        let mut create = Button::new("new-memory-create").primary().label("Create");
        if valid {
            create = create.on_click(move |_event, window, cx| {
                let path = path.clone();
                confirming
                    .update(cx, |app, cx| app.create_memory(&path, cx))
                    .ok();
                window.close_dialog(cx);
            });
        }
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
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "A new Memory document in {shown}. It is saved as a draft, and exists for the Project once its Review is merged."
                    )),
            )
            .child(Input::new(&self.name))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("new-memory-cancel").label("Cancel"))
                    .child(create),
            )
    }
}
