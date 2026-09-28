//! The dialogs Memory needs: one to confirm something that cannot be undone by
//! a click, and one to ask for a name.
//!
//! Both are dialogs rather than panes, for the reason Project settings is: a
//! reader who loses the document to a form has to work out how to get it back.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::modal;
use crate::engine::DocumentEdit;
use crate::ui::{self, Typography};

/// What a dialog does when it is confirmed. An enumeration rather than a
/// closure: the dialog says what it is for, and the application keeps the calls
/// it makes.
#[derive(Clone)]
pub enum DialogAction {
    /// Propose that every document a set of rows stands for be deleted: one
    /// document is a set of one, and a folder is every document below it.
    DeleteDocuments { paths: Vec<String> },
    /// Throw away every draft a set of rows carries.
    DiscardDrafts { paths: Vec<String> },
    /// Move the Project's Memory back to the standard location.
    ResetStorage { project_id: String, revision: i64 },
    /// Build the Project's cache again.
    ClearCache { project_id: String, revision: i64 },
    /// Sync the Project's drafts now.
    SyncNow { project_id: String },
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
        let footer_view = view.clone();
        modal::open(
            window,
            cx,
            title,
            modal::NARROW,
            move |dialog, _window, cx| {
                let confirm = modal::primary("dialog-confirm", footer_view.read(cx).confirm, true)
                    .on_click({
                        let view = footer_view.clone();
                        move |_event, window, cx| {
                            let _ = view.update(cx, |dialog, cx| dialog.confirm(window, cx));
                        }
                    })
                    .into_any_element();
                dialog
                    .content({
                        let view = view.clone();
                        move |content, _window, _cx| content.child(view.clone())
                    })
                    .footer(modal::footer(Some(modal::cancel("Cancel", true)), confirm))
            },
        );
    }

    /// Does the thing the dialog was opened for, and leaves.
    fn confirm(&mut self, window: &mut Window, cx: &mut App) {
        let action = self.action.clone();
        let _ = self
            .app
            .update(cx, |app, cx| app.run_dialog_action(action, cx));
        window.close_dialog(cx);
    }
}

impl Render for ConfirmDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .text_style(&ui::BODY)
            .text_color(cx.theme().foreground)
            .child(self.message.clone())
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
        let footer_view = view.clone();
        modal::open(
            window,
            cx,
            "Rename folder",
            modal::NARROW,
            move |dialog, _window, cx| {
                let ready = footer_view.read(cx).ready(cx);
                let confirm = modal::primary("rename-folder-confirm", "Rename", ready)
                    .on_click({
                        let view = footer_view.clone();
                        move |_event, window, cx| {
                            let _ = view.update(cx, |dialog, cx| dialog.rename(window, cx));
                        }
                    })
                    .into_any_element();
                dialog
                    .content({
                        let view = view.clone();
                        move |content, _window, _cx| content.child(view.clone())
                    })
                    .footer(modal::footer(Some(modal::cancel("Cancel", true)), confirm))
            },
        );
    }

    /// A folder name is one path component, and nothing else.
    fn ready(&self, cx: &App) -> bool {
        let name = self.name.read(cx).value().trim().to_owned();
        !name.is_empty() && name != "." && name != ".." && !name.contains('/')
    }

    fn rename(&mut self, window: &mut Window, cx: &mut App) {
        let name = self.name.read(cx).value().trim().to_owned();
        if !self.ready(cx) {
            return;
        }
        let folder = self.folder.clone();
        let _ = self
            .app
            .update(cx, |app, cx| app.rename_folder(&folder, &name, cx));
        window.close_dialog(cx);
    }
}

impl Render for RenameFolderDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Every memory in {} moves with the folder, keeping its relative path. Each move is saved as a draft.",
                        self.folder
                    )),
            )
            .child(Input::new(&self.name))
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
        let footer_view = view.clone();
        modal::open(
            window,
            cx,
            "Rename",
            modal::NARROW,
            move |dialog, _window, cx| {
                let ready = footer_view.read(cx).ready(cx);
                let confirm = modal::primary("rename-confirm", "Rename", ready)
                    .on_click({
                        let view = footer_view.clone();
                        move |_event, window, cx| {
                            let _ = view.update(cx, |dialog, cx| dialog.rename(window, cx));
                        }
                    })
                    .into_any_element();
                dialog
                    .content({
                        let view = view.clone();
                        move |content, _window, _cx| content.child(view.clone())
                    })
                    .footer(modal::footer(Some(modal::cancel("Cancel", true)), confirm))
            },
        );
    }

    /// A file name is one path component, and nothing else: the daemon
    /// validates the path itself and reports what it decided.
    fn ready(&self, cx: &App) -> bool {
        let typed = self.name.read(cx).value().trim().to_owned();
        !typed.is_empty() && typed != "." && typed != ".." && !typed.contains('/')
    }

    fn path(&self, cx: &App) -> String {
        let typed = self.name.read(cx).value().trim().to_owned();
        if self.parent.is_empty() {
            typed
        } else {
            format!("{}/{typed}", self.parent)
        }
    }

    fn rename(&mut self, window: &mut Window, cx: &mut App) {
        if !self.ready(cx) {
            return;
        }
        let path = self.path(cx);
        let edit = self.edit.clone();
        let _ = self
            .app
            .update(cx, |app, cx| app.rename_document(edit, &path, cx));
        window.close_dialog(cx);
    }
}

impl Render for RenameDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child("The rename is saved as a draft, and takes effect for the Project after review and merge."),
            )
            .child(Input::new(&self.name))
    }
}
