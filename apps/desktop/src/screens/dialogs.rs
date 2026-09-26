//! The dialogs Memory needs: one to confirm something that cannot be undone by
//! a click, and one to ask for a name.
//!
//! Both are dialogs rather than panes, for the reason Project settings is: a
//! reader who loses the document to a form has to work out how to get it back.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::engine::DocumentEdit;
use crate::ui::{self, Typography};

/// What a dialog does when it is confirmed. An enumeration rather than a
/// closure: the dialog says what it is for, and the application keeps the calls
/// it makes.
#[derive(Clone)]
pub enum DialogAction {
    /// Propose that a document be deleted.
    DeleteDocument { edit: DocumentEdit, path: String },
    /// Move the Project's Memory back to the standard location.
    ResetStorage { project_id: String, revision: i64 },
    /// Build the Project's cache again.
    ClearCache { project_id: String, revision: i64 },
    /// Sync the Project's drafts now.
    SyncNow { project_id: String },
    /// Propose that every document below a folder be deleted.
    DeleteFolder { folder: String },
    /// Throw away every draft below a folder.
    DiscardFolder { folder: String },
}

pub struct ConfirmDialog {
    /// The entity the buttons talk to.
    app: WeakEntity<DesktopApp>,
    message: String,
    confirm: &'static str,
    action: DialogAction,
}

impl ConfirmDialog {
    /// Opens one, titled and sized like the rest of this client's dialogs.
    pub fn open(
        app: WeakEntity<DesktopApp>,
        title: &'static str,
        message: String,
        confirm: &'static str,
        action: DialogAction,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = cx.new(|_| Self {
            app,
            message,
            confirm,
            action,
        });
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let view = view.clone();
            dialog
                .title(title)
                .w(px(460.))
                .keyboard(true)
                .content(move |content, _window, _cx| content.child(view.clone()))
                .footer(div())
                .footer(div())
        });
    }
}

impl Render for ConfirmDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = _cx.theme();
        let confirming = self.app.clone();
        let action = self.action.clone();
        div()
            .v_flex()
            .gap_4()
            .child(
                div()
                    .text_style(&ui::BODY)
                    .text_color(theme.foreground)
                    .child(self.message.clone()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("dialog-cancel").label("Cancel"))
                    .child(
                        Button::new("dialog-confirm")
                            .primary()
                            .label(self.confirm)
                            .on_click(move |_event, window, cx| {
                                confirming
                                    .update(cx, |app, cx| app.run_dialog_action(action.clone(), cx))
                                    .ok();
                                window.close_dialog(cx);
                            }),
                    ),
            )
    }
}

/// A dialog with one field, for renaming a folder. The documents below it move
/// with it, keeping their relative paths.
pub struct RenameFolderDialog {
    app: WeakEntity<DesktopApp>,
    folder: String,
    name: Entity<InputState>,
}

impl RenameFolderDialog {
    pub fn open(
        app: WeakEntity<DesktopApp>,
        folder: &str,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
    ) {
        let current = folder.rsplit('/').next().unwrap_or(folder).to_owned();
        let field = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let folder = folder.to_owned();
        let view = cx.new(|_| Self {
            app,
            folder,
            name: field,
        });
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let view = view.clone();
            dialog
                .title("Rename folder")
                .w(px(460.))
                .keyboard(true)
                .content(move |content, _window, _cx| content.child(view.clone()))
                .footer(div())
                .footer(div())
        });
    }
}

impl Render for RenameFolderDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = self.name.read(cx).value().trim().to_owned();
        let valid = !name.is_empty() && name != "." && name != ".." && !name.contains('/');
        let confirming = self.app.clone();
        let folder = self.folder.clone();
        let mut confirm = Button::new("rename-folder-confirm")
            .primary()
            .label("Rename");
        if valid {
            confirm = confirm.on_click(move |_event, window, cx| {
                let folder = folder.clone();
                let name = name.clone();
                confirming
                    .update(cx, |app, cx| app.rename_folder(&folder, &name, cx))
                    .ok();
                window.close_dialog(cx);
            });
        }
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "Every memory in {} moves with the folder, keeping its relative path. Each move is saved as a draft.",
                        self.folder
                    )),
            )
            .child(Input::new(&self.name))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("rename-folder-cancel").label("Cancel"))
                    .child(confirm),
            )
    }
}
/// A dialog with one field, for renaming a document.
pub struct RenameDialog {
    app: WeakEntity<DesktopApp>,
    edit: DocumentEdit,
    /// The directory the document lives in, which does not change: this renames
    /// the last path component rather than moving the file.
    parent: String,
    name: Entity<InputState>,
}

impl RenameDialog {
    pub fn open(
        app: WeakEntity<DesktopApp>,
        path: &str,
        edit: DocumentEdit,
        window: &mut Window,
        cx: &mut Context<DesktopApp>,
    ) {
        let (parent, name) = match path.rsplit_once('/') {
            Some((parent, name)) => (parent.to_owned(), name.to_owned()),
            None => (String::new(), path.to_owned()),
        };
        let field = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let view = cx.new(|_| Self {
            app,
            edit,
            parent,
            name: field,
        });
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let view = view.clone();
            dialog
                .title("Rename")
                .w(px(460.))
                .keyboard(true)
                .content(move |content, _window, _cx| content.child(view.clone()))
                .footer(div())
                .footer(div())
        });
    }
}

impl Render for RenameDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let typed = self.name.read(cx).value().trim().to_owned();
        // What a file name may not be, which is the whole of what this checks:
        // the daemon validates the path itself and reports what it decided.
        let valid = !typed.is_empty() && typed != "." && typed != ".." && !typed.contains('/');
        let path = if self.parent.is_empty() {
            typed.clone()
        } else {
            format!("{}/{typed}", self.parent)
        };
        let confirming = self.app.clone();
        let edit = self.edit.clone();
        let mut confirm = Button::new("rename-confirm").primary().label("Rename");
        if valid {
            confirm = confirm.on_click(move |_event, window, cx| {
                let path = path.clone();
                let edit = edit.clone();
                confirming
                    .update(cx, |app, cx| app.rename_document(edit.clone(), &path, cx))
                    .ok();
                window.close_dialog(cx);
            });
        }
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(theme.muted_foreground)
                    .child("The rename is saved as a draft, and takes effect for the Project after review and merge."),
            )
            .child(Input::new(&self.name))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("rename-cancel").label("Cancel"))
                    .child(confirm),
            )
    }
}
