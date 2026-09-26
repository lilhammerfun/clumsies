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
use crate::engine::ProjectStorage;
use crate::screens::dialogs::{ConfirmDialog, DialogAction};
use crate::ui::{self, Typography};

/// Everything the dialog shows. It is read before the dialog opens, because
/// each line is a socket call to the daemon and a dialog that arrives while it
/// is still loading flickers.
pub struct ProjectSettings {
    pub project: String,
    /// The Project the commands act on.
    pub project_id: String,
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
}

impl ProjectSettingsDialog {
    pub fn new(app: WeakEntity<DesktopApp>, settings: ProjectSettings) -> Self {
        Self { app, settings }
    }
}

impl Render for ProjectSettingsDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = _cx.theme();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut heading = |text: &str, rows: &mut Vec<AnyElement>, cx: &App| {
            rows.push(
                div()
                    .pt(px(ui::SPACE_SM))
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child(text.to_owned())
                    .into_any_element(),
            );
        };

        heading("Memory", &mut rows, _cx);
        match &self.settings.storage {
            Ok(storage) => {
                rows.push(entry("Location", storage.location.clone(), _cx));
                rows.push(entry("Used", bytes(storage.used_bytes), _cx));
                rows.push(entry("Status", storage.status.to_owned(), _cx));
                if let Some(diagnostic) = &storage.diagnostic {
                    rows.push(entry("Diagnostic", diagnostic.clone(), _cx));
                }
            }
            Err(error) => rows.push(
                div()
                    .text_style(&ui::BODY)
                    .text_color(theme.danger)
                    .child(error.clone())
                    .into_any_element(),
            ),
        }

        heading("Engine", &mut rows, _cx);
        rows.push(entry("Project", self.settings.project.clone(), _cx));
        rows.push(entry(
            "Server",
            self.settings
                .server
                .clone()
                .unwrap_or_else(|| "not connected".to_owned()),
            _cx,
        ));
        rows.push(entry("Daemon", self.settings.daemon.clone(), _cx));
        if let Some(log_dir) = &self.settings.log_dir {
            rows.push(entry("Logs", log_dir.clone(), _cx));
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

/// One line of the dialog: what it is, and what it says.
fn entry(label: &str, value: String, cx: &App) -> AnyElement {
    div()
        .h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(px(88.))
                .flex_shrink_0()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_style(&ui::BODY)
                .child(value),
        )
        .into_any_element()
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
