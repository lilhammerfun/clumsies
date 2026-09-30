//! The Project settings dialog: what this machine holds for the open Project.
//!
//! macOS shows the same read-outs by replacing the work with a settings pane,
//! which leaves a reader looking at settings with no obvious way back to the
//! document they were reading. A dialog is a surface with its own close, so the
//! work is never taken away from under them.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::*;
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::modal;
use crate::engine::{self, ProjectStorage};
use crate::screens::dialogs::{ConfirmDialog, DialogAction};
use crate::ui::{self, Typography};
use gpui_kit::component::Disableable;
use gpui_kit::component::input::{Input, InputState};

/// Everything the dialog shows. It is read before the dialog opens, because
/// each line is a socket call to the daemon and a dialog that arrives while it
/// is still loading flickers.
pub struct ProjectSettings {
    pub project: String,
    /// The Project the commands act on.
    pub project_id: String,
    /// What the space says about itself, as the Server last answered.
    pub description: String,
    /// The revision those two were read at, which the next write carries.
    pub revision: i64,
    /// Where this space keeps its guidelines on this machine, when the daemon
    /// was told. It is read rather than written: the daemon's only call for it
    /// carries the session, which it does not hand back.
    pub guidelines: Option<String>,
    /// Where the Project's Memory is; the daemon's own sentence when it could
    /// not say.
    pub storage: Result<ProjectStorage, String>,
    pub server: Option<String>,
    pub daemon: String,
    pub log_dir: Option<String>,
}

pub struct ProjectSettingsDialog {
    /// The entity the commands talk to.
    app: WeakEntity<DesktopApp>,
    settings: ProjectSettings,
    connections: Option<engine::Connections>,
    /// The Memory space section's own form: what the space is called, and what
    /// it says about itself, while the reader is changing either.
    editing: bool,
    name: Entity<InputState>,
    description: Entity<InputState>,
    saving: bool,
    space_error: Option<String>,
    space_notice: Option<String>,
    busy: bool,
    connection_error: Option<String>,
    connection_notice: Option<String>,
}

impl ProjectSettingsDialog {
    pub fn new(
        app: WeakEntity<DesktopApp>,
        settings: ProjectSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).default_value(settings.project.clone()));
        let description =
            cx.new(|cx| InputState::new(window, cx).default_value(settings.description.clone()));
        let mut view = Self {
            app,
            settings,
            connections: None,
            editing: false,
            name,
            description,
            saving: false,
            space_error: None,
            space_notice: None,
            busy: false,
            connection_error: None,
            connection_notice: None,
        };
        view.refresh_connections(cx);
        view
    }

    /// The Memory space section: what the space is called, what it says about
    /// itself, and where its guidelines live on this machine.
    fn memory_space(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![modal::heading("Memory space", cx)];
        if !self.editing {
            rows.push(modal::entry(
                "Name",
                div()
                    .text_style(&ui::BODY)
                    .child(self.settings.project.clone()),
                cx,
            ));
            rows.push(modal::entry(
                "About",
                div()
                    .text_style(&ui::BODY)
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.settings.description.is_empty() {
                        "Not set".to_owned()
                    } else {
                        self.settings.description.clone()
                    }),
                cx,
            ));
            rows.push(modal::entry(
                "Guidelines",
                div()
                    .text_style(&ui::BODY)
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        self.settings
                            .guidelines
                            .clone()
                            .unwrap_or_else(|| "The default location".to_owned()),
                    ),
                cx,
            ));
            rows.push(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("memory-space-edit")
                            .label("Edit…")
                            .on_click(cx.listener(|dialog, _event, _window, cx| {
                                dialog.editing = true;
                                dialog.space_error = None;
                                dialog.space_notice = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        } else {
            rows.push(Input::new(&self.name).into_any_element());
            rows.push(Input::new(&self.description).into_any_element());
            rows.push(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("memory-space-save")
                            .primary()
                            .label("Save")
                            .disabled(self.saving || !self.space_ready(cx))
                            .on_click(cx.listener(|dialog, _event, _window, cx| {
                                dialog.save_space(cx);
                            })),
                    )
                    .child(
                        Button::new("memory-space-cancel")
                            .label("Cancel")
                            .disabled(self.saving)
                            .on_click(cx.listener(|dialog, _event, _window, cx| {
                                dialog.editing = false;
                                dialog.name.update(cx, |state, cx| {
                                    state.set_value(dialog.settings.project.clone(), _window, cx)
                                });
                                dialog.description.update(cx, |state, cx| {
                                    state.set_value(
                                        dialog.settings.description.clone(),
                                        _window,
                                        cx,
                                    )
                                });
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = &self.space_error {
            rows.push(
                div()
                    .text_style(&ui::BODY)
                    .text_color(cx.theme().danger)
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        if let Some(notice) = &self.space_notice {
            rows.push(
                div()
                    .text_style(&ui::BODY)
                    .text_color(cx.theme().muted_foreground)
                    .child(notice.clone())
                    .into_any_element(),
            );
        }
        rows
    }

    /// A space with no name cannot be told apart from the next one.
    fn space_ready(&self, cx: &Context<Self>) -> bool {
        !self.name.read(cx).value().trim().is_empty()
    }

    fn save_space(&mut self, cx: &mut Context<Self>) {
        if self.saving || !self.space_ready(cx) {
            return;
        }
        let project_id = self.settings.project_id.clone();
        let revision = self.settings.revision;
        let name = self.name.read(cx).value().trim().to_owned();
        let description = self.description.read(cx).value().trim().to_owned();
        self.saving = true;
        self.space_error = None;
        self.space_notice = None;
        cx.notify();
        let app = self.app.clone();
        let task = cx.background_executor().spawn({
            let project_id = project_id.clone();
            let name = name.clone();
            let description = description.clone();
            async move { engine::update_project(&project_id, &name, &description, revision) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |dialog, cx| {
                dialog.saving = false;
                match result {
                    Ok(project) => {
                        dialog.settings.project = project.name;
                        dialog.settings.description = project.description;
                        dialog.settings.revision = project.revision;
                        dialog.editing = false;
                        dialog.space_notice = Some("Saved.".to_owned());
                        // The filter and the headers name the space, so the
                        // window re-reads the list it draws them from.
                        let _ = app.update(cx, |app, cx| app.memory_space_changed(cx));
                    }
                    Err(error) => dialog.space_error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_connections(&mut self, cx: &mut Context<Self>) {
        self.connection_action(|| Ok(()), None, cx);
    }

    fn connection_action(
        &mut self,
        action: impl FnOnce() -> Result<(), String> + Send + 'static,
        notice: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.connection_error = None;
        self.connection_notice = None;
        let project = self.settings.project_id.clone();
        let notice = notice.map(str::to_owned);
        let work = cx.background_executor().spawn(async move {
            let result = action();
            let connections = engine::connections(&project);
            (result, connections)
        });
        cx.spawn(async move |this, cx| {
            let (result, connections) = work.await;
            this.update(cx, |view, cx| {
                view.busy = false;
                match connections {
                    Ok(connections) => view.connections = Some(connections),
                    Err(error) => view.connection_error = Some(error),
                }
                match result {
                    Ok(()) => view.connection_notice = notice,
                    Err(error) => view.connection_error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Link work folder".into()),
        });
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = picker.await;
            this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            let project = view.settings.project_id.clone();
                            view.connection_action(move || engine::bind_workspace(&project, &path),
                                Some("Folder linked. Open your AI tool in this folder to use this project's Memory."), cx);
                        }
                    }
                    Ok(Ok(None)) => {},
                    Ok(Err(error)) => view.connection_error = Some(error.to_string()),
                    Err(error) => view.connection_error = Some(error.to_string()),
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    fn connection_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![modal::heading("Work folders", cx)];
        rows.push(ui::message(
            "Link a local folder so AI tools opened there use this project's Memory.",
            cx.theme().muted_foreground,
        ));
        if let Some(connections) = &self.connections {
            for (index, binding) in connections.bindings.iter().enumerate() {
                let removing = binding.clone();
                rows.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .child(binding.workspace_root.clone()),
                        )
                        .child(
                            Button::new(("unlink-work-folder", index))
                                .label("Unlink")
                                .disabled(self.busy)
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    let binding = removing.clone();
                                    view.connection_action(
                                        move || engine::unbind_workspace(&binding),
                                        Some("Folder unlinked."),
                                        cx,
                                    );
                                })),
                        )
                        .into_any_element(),
                );
            }
            if connections.bindings.is_empty() {
                rows.push(ui::message(
                    "No work folders linked.",
                    cx.theme().muted_foreground,
                ));
            }
        }
        rows.push(
            div()
                .h_flex()
                .gap_2()
                .child(
                    Button::new("link-work-folder")
                        .label("Link folder…")
                        .disabled(self.busy)
                        .on_click(cx.listener(|view, _, _, cx| view.choose_folder(cx))),
                )
                .child(
                    Button::new("refresh-connections")
                        .label("Refresh")
                        .disabled(self.busy)
                        .on_click(cx.listener(|view, _, _, cx| view.refresh_connections(cx))),
                )
                .into_any_element(),
        );
        if self.busy {
            rows.push(ui::message(
                "Updating connections…",
                cx.theme().muted_foreground,
            ));
        }
        if let Some(error) = &self.connection_error {
            rows.push(ui::message(error.clone(), cx.theme().danger));
        }
        if let Some(notice) = &self.connection_notice {
            rows.push(ui::message(notice.clone(), cx.theme().muted_foreground));
        }
        rows
    }
}

impl Render for ProjectSettingsDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = self.connection_rows(_cx);
        // The colours are read once and copied, because the sections below
        // borrow the context mutably to build their own controls.
        let theme = _cx.theme();
        let danger = theme.danger;

        rows.extend(self.memory_space(_cx));
        rows.push(modal::heading("Memory", _cx));
        match &self.settings.storage {
            Ok(storage) => {
                rows.push(modal::entry("Location", storage.location.clone(), _cx));
                rows.push(modal::entry("Used", bytes(storage.used_bytes), _cx));
                rows.push(modal::entry("Status", storage.status.to_owned(), _cx));
                if let Some(diagnostic) = &storage.diagnostic {
                    rows.push(modal::entry("Diagnostic", diagnostic.clone(), _cx));
                }
            }
            Err(error) => rows.push(
                div()
                    .text_style(&ui::BODY)
                    .text_color(danger)
                    .child(error.clone())
                    .into_any_element(),
            ),
        }

        rows.push(modal::heading("Engine", _cx));
        rows.push(modal::entry("Project", self.settings.project.clone(), _cx));
        rows.push(modal::entry(
            "Server",
            self.settings
                .server
                .clone()
                .unwrap_or_else(|| "not connected".to_owned()),
            _cx,
        ));
        rows.push(modal::entry("Daemon", self.settings.daemon.clone(), _cx));
        if let Some(log_dir) = &self.settings.log_dir {
            rows.push(modal::entry("Logs", log_dir.clone(), _cx));
        }

        // What can be done about it. The two that change where Memory lives ask
        // first: they move files, and a dialog is the smallest place to say so.
        if let Ok(storage) = &self.settings.storage {
            let project_id = self.settings.project_id.clone();
            let syncing = self.app.clone();
            let resetting = self.app.clone();
            let clearing = self.app.clone();
            let sync_project = project_id.clone();
            let reset_project = project_id.clone();
            let clear_project = project_id.clone();
            let revision = storage.location_revision;
            rows.push(
                div()
                    .h_flex()
                    .gap_2()
                    .pt(px(ui::SPACE_SM))
                    .child(
                        Button::new("storage-sync").label("Sync now").on_click(
                            move |_event, _window, cx| {
                                syncing
                                    .update(cx, |app, cx| {
                                        app.run_dialog_action(
                                            DialogAction::SyncNow {
                                                project_id: sync_project.clone(),
                                            },
                                            cx,
                                        )
                                    })
                                    .ok();
                            },
                        ),
                    )
                    .child(
                        Button::new("storage-reset").label("Reset location…").on_click(
                            move |_event, window, cx| {
                                ConfirmDialog::open(
                                    resetting.clone(),
                                    "Reset to the standard location?",
                                    "Clumsies will move this Project's Memory back to the standard location for this machine.".to_owned(),
                                    "Reset",
                                    DialogAction::ResetStorage {
                                        project_id: reset_project.clone(),
                                        revision,
                                    },
                                    window,
                                    cx,
                                );
                            },
                        ),
                    )
                    .child(
                        Button::new("storage-clear").label("Clear cache…").on_click(
                            move |_event, window, cx| {
                                ConfirmDialog::open(
                                    clearing.clone(),
                                    "Clear this Project's cache?",
                                    "Drafts and settings are preserved. Commit generations and the search index will be built again.".to_owned(),
                                    "Clear cache",
                                    DialogAction::ClearCache {
                                        project_id: clear_project.clone(),
                                        revision,
                                    },
                                    window,
                                    cx,
                                );
                            },
                        ),
                    )
                    .into_any_element(),
            );
        }

        div().v_flex().w_full().gap_2().children(rows)
    }
}

/// What a reader can compare at a glance: bytes are not readable numbers.
fn bytes(value: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024. && unit + 1 < UNITS.len() {
        size /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}
