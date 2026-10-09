//! Settings: whose account this window is signed in as, what it signs in with,
//! and what this machine holds for it.
//!
//! The information is the macOS client's, read from its SettingsWindowView,
//! SettingsNavigation and AccountSecurityView: the account block is the Account
//! pane, then General, Agents, Support, and the Organization's administration
//! when the account's role grants it. What the account signs in with — the
//! username, the local password, the identity provider — each opens its form in
//! place, and both changes answer with a fresh session, because changing a
//! password signs every other session out and connecting a provider re-issues
//! this one.
//!
//! The surface is not macOS's. Its grouped forms are Apple's HIG, which says
//! nothing about a Linux or Windows window; the surface is the component
//! library's `setting` element instead, which is the same shape Zed's settings
//! have — a searchable navigation column, pages of groups, and one row per
//! setting with its control at the end of the row. That is also the rule
//! DESIGN.md states: the library first, and a second implementation of a list
//! of settings is a second list to keep in step.
//!
//! What is not built, named here rather than left to be discovered: the Organization's
//! administration panes (its name, members, Projects, access and audit), and
//! General's language and update controls, because this client has neither a
//! translation nor an updater.

use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::Icon;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::setting::{
    SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::components::modal;
use crate::engine::{self, Account, Credentials};
use crate::sign_in::LoginMethods;
use crate::ui::{self, Typography};

/// Which of the Account pane's forms is open — macOS's `Action`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Setting or changing the local password.
    Password,
    /// Connecting an identity provider to this account.
    Connect,
}

/// Everything the dialog shows that is read once. A dialog that arrives while
/// it is still loading flickers, which is the rule the Project settings dialog
/// follows too.
pub struct Facts {
    pub account: Result<Account, String>,
    pub log_dir: Option<String>,
    pub client: &'static str,
}

pub struct SettingsDialog {
    /// The window, which owns the session the Server re-issues: a change is
    /// reported to it, not kept here.
    app: WeakEntity<DesktopApp>,
    facts: Facts,
    /// What the account signs in with, and what this Server offers. Both are
    /// re-read after a change, because a change answers with the new state.
    credentials: Result<Credentials, String>,
    methods: Result<LoginMethods, String>,
    action: Option<Action>,
    username: Entity<InputState>,
    current_password: Entity<InputState>,
    password: Entity<InputState>,
    confirmation: Entity<InputState>,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    agents: Option<clumsiesd::DaemonAgentAdapterSettings>,
    agents_busy: bool,
    agents_mutating: bool,
    agents_error: Option<String>,
    codex_status: Option<Result<clumsiesd::DaemonCodexPluginStatus, String>>,
    clear_fields: bool,
    clear_secrets: bool,
    pub confirm_close: bool,
    support_busy: bool,
    support_error: Option<String>,
    _field_subscriptions: Vec<Subscription>,
}

impl SettingsDialog {
    pub fn new(
        app: WeakEntity<DesktopApp>,
        facts: Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut field = |cx: &mut Context<Self>, placeholder: &str, secret: bool| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(secret)
            })
        };
        let origin = engine::configured_server_url().unwrap_or_default();
        let mut view = Self {
            app,
            facts,
            credentials: engine::credentials(),
            methods: crate::sign_in::login_methods(&origin),
            action: None,
            username: field(cx, "Username", false),
            current_password: field(cx, "Current password", true),
            password: field(cx, "New password", true),
            confirmation: field(cx, "Confirm new password", true),
            busy: false,
            error: None,
            notice: None,
            agents: None,
            agents_busy: false,
            agents_mutating: false,
            agents_error: None,
            codex_status: None,
            clear_fields: false,
            clear_secrets: false,
            confirm_close: false,
            support_busy: false,
            support_error: None,
            _field_subscriptions: Vec::new(),
        };
        view._field_subscriptions = [
            &view.username,
            &view.current_password,
            &view.password,
            &view.confirmation,
        ]
        .iter()
        .map(|field| cx.observe(*field, |_, _, cx| cx.notify()))
        .collect();
        view.update_agents(None, cx);
        view
    }

    pub fn working(&self) -> bool {
        self.busy || self.agents_mutating || self.support_busy
    }

    pub fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.working() {
            return false;
        }
        let dirty = self.action.is_some()
            && [
                &self.username,
                &self.current_password,
                &self.password,
                &self.confirmation,
            ]
            .iter()
            .any(|field| !field.read(cx).value().is_empty());
        if dirty {
            self.confirm_close = true;
            cx.notify();
            return false;
        }
        true
    }

    pub fn keep_editing(&mut self, cx: &mut Context<Self>) {
        self.confirm_close = false;
        cx.notify();
    }

    pub fn cancel_action(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.clear_form();
        cx.notify();
    }

    pub fn confirm_action(&mut self, cx: &mut Context<Self>) {
        if let Some(action) = self.action
            && self.action_ready(action, cx)
            && !self.busy
        {
            self.run(action, cx);
        }
    }

    fn password_set(&self) -> bool {
        self.credentials
            .as_ref()
            .is_ok_and(|credentials| credentials.password_set)
    }

    fn has_username(&self) -> bool {
        self.credentials
            .as_ref()
            .is_ok_and(|credentials| credentials.username.is_some())
    }

    /// The whole surface: the library's own settings element, which is a
    /// searchable navigation column beside pages of rows.
    pub fn surface(&self, cx: &mut Context<Self>) -> AnyElement {
        Settings::new("settings")
            .sidebar_width(px(220.))
            .default_selected_index(SelectIndex {
                page_ix: 1,
                group_ix: None,
            })
            .page(self.account_page(cx))
            .page(self.general_page(cx))
            .page(self.agents_page(cx))
            .page(self.support_page(cx))
            .into_any_element()
    }

    fn account_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let mut page = SettingPage::new("Account")
            .icon(Icon::default().path("icons/circle-user.svg"))
            .description(
                self.facts
                    .account
                    .as_ref()
                    .map(|a| format!("{} · {}", a.user.identity_label(), a.organization))
                    .unwrap_or_else(|_| "How you sign in".into()),
            );

        if let Err(error) = &self.credentials {
            page = page.group(SettingGroup::new().item(SettingItem::new(
                "Account",
                SettingField::<SharedString>::render({
                    let error = error.clone();
                    move |_options, _window, cx| {
                        div()
                            .text_style(&ui::BODY)
                            .text_color(cx.theme().danger)
                            .child(error.clone())
                    }
                }),
            )));
        } else {
            let credentials = self.credentials.as_ref().expect("checked above");
            let username = credentials
                .username
                .clone()
                .unwrap_or_else(|| "Not set".to_owned());
            let mut sign_in = SettingGroup::new().title("Sign in").item(SettingItem::new(
                "Username",
                SettingField::<SharedString>::render(move |_options, _window, cx| {
                    div()
                        .text_style(&ui::BODY)
                        .text_color(cx.theme().muted_foreground)
                        .child(username.clone())
                }),
            ));

            if self.passwords() {
                let label = if credentials.password_set {
                    "Change password…"
                } else {
                    "Set password…"
                };
                let dialog = cx.entity().downgrade();
                let enabled = !self.busy && self.action.is_none();
                sign_in = sign_in.item(SettingItem::new(
                    "Password",
                    SettingField::<SharedString>::render(move |_options, _window, _cx| {
                        let dialog = dialog.clone();
                        Button::new("settings-password")
                            .label(label)
                            .disabled(!enabled)
                            .on_click(move |_event, _window, cx| {
                                dialog
                                    .update(cx, |dialog, cx| {
                                        dialog.begin(Action::Password, cx);
                                    })
                                    .ok();
                            })
                    }),
                ));
            }

            let google = self.methods.as_ref().is_ok_and(|methods| methods.google);
            let has_provider = self
                .methods
                .as_ref()
                .is_ok_and(|methods| methods.oidc_enabled)
                || credentials.oidc_email.is_some();
            if has_provider {
                let title = if google {
                    "Google account"
                } else {
                    "Single sign-on"
                };
                let email = credentials.oidc_email.clone();
                let dialog = cx.entity().downgrade();
                let enabled = !self.busy && self.action.is_none();
                sign_in = sign_in.item(SettingItem::new(
                    title,
                    SettingField::<SharedString>::render(
                        move |_options, _window, cx| match &email {
                            Some(email) => div()
                                .text_style(&ui::BODY)
                                .text_color(cx.theme().muted_foreground)
                                .child(email.clone())
                                .into_any_element(),
                            None => {
                                let dialog = dialog.clone();
                                div()
                                    .h_flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_style(&ui::BODY)
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Not connected"),
                                    )
                                    .child(
                                        Button::new("settings-connect")
                                            .label("Connect account")
                                            .disabled(!enabled)
                                            .on_click(move |_event, _window, cx| {
                                                dialog
                                                    .update(cx, |dialog, cx| {
                                                        dialog.begin(Action::Connect, cx);
                                                    })
                                                    .ok();
                                            }),
                                    )
                                    .into_any_element()
                            }
                        },
                    ),
                ));
            }
            page = page.group(sign_in);
        }

        if let Some(action) = self.action {
            page = page.group(self.action_group(action, cx));
        }
        if let Some(error) = &self.error {
            let error = error.clone();
            page = page.group(SettingGroup::new().item(SettingItem::new(
                "Last attempt",
                SettingField::<SharedString>::render(move |_options, _window, cx| {
                    div()
                        .text_style(&ui::BODY)
                        .text_color(cx.theme().danger)
                        .child(error.clone())
                }),
            )));
        }
        if let Some(notice) = &self.notice {
            let notice = notice.clone();
            page = page.group(SettingGroup::new().item(SettingItem::new(
                "Last change",
                SettingField::<SharedString>::render(move |_options, _window, cx| {
                    div()
                        .text_style(&ui::BODY)
                        .text_color(cx.theme().muted_foreground)
                        .child(notice.clone())
                }),
            )));
        }
        page
    }

    /// The form an action opens in place, as rows of the same shape as the rest
    /// of the page rather than a sheet: what the action needs, and the sentence
    /// explaining it.
    fn action_group(&self, action: Action, cx: &mut Context<Self>) -> SettingGroup {
        let title = match action {
            Action::Password if self.password_set() => "Change password",
            Action::Password => "Set password",
            Action::Connect => "Verify your identity",
        };
        let busy = self.busy;
        let mut group = SettingGroup::new().title(title);
        if self.password_set() {
            let field = self.current_password.clone();
            group = group.item(SettingItem::new(
                "Current password",
                SettingField::<SharedString>::render(move |_options, _window, _cx| {
                    Input::new(&field).disabled(busy)
                }),
            ));
        }
        if action == Action::Password {
            if !self.has_username() {
                let field = self.username.clone();
                group = group.item(SettingItem::new(
                    "Username",
                    SettingField::<SharedString>::render(move |_options, _window, _cx| {
                        Input::new(&field).disabled(busy)
                    }),
                ));
            }
            let password = self.password.clone();
            group = group.item(SettingItem::new(
                "New password",
                SettingField::<SharedString>::render(move |_options, _window, _cx| {
                    Input::new(&password).disabled(busy)
                }),
            ));
            let confirmation = self.confirmation.clone();
            group = group.item(
                SettingItem::new(
                    "Confirm new password",
                    SettingField::<SharedString>::render(move |_options, _window, _cx| {
                        Input::new(&confirmation).disabled(busy)
                    }),
                )
                .description("At least 15 characters. Other sessions will be signed out."),
            );
        } else {
            group = group.description("Continue in your browser to connect your account.");
        }
        let view = cx.entity().downgrade();
        let ready = !busy && self.action_ready(action, cx);
        let confirm = match action {
            Action::Password if self.password_set() => "Change password",
            Action::Password => "Set password",
            Action::Connect => "Continue",
        };
        group.item(SettingItem::new(
            "",
            SettingField::<SharedString>::render(move |_, _, _| {
                let cancel = view.clone();
                let submit = view.clone();
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("account-cancel")
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(move |_, _, cx| {
                                cancel.update(cx, |view, cx| view.cancel_action(cx)).ok();
                            }),
                    )
                    .child(
                        Button::new("account-confirm")
                            .primary()
                            .label(confirm)
                            .disabled(!ready)
                            .loading(busy)
                            .on_click(move |_, _, cx| {
                                submit.update(cx, |view, cx| view.confirm_action(cx)).ok();
                            }),
                    )
            }),
        ))
    }

    /// General owns app information; account and connection facts belong to their own pages.
    fn general_page(&self, _cx: &mut Context<Self>) -> SettingPage {
        let version = self.facts.client.to_owned();
        SettingPage::new("General")
            .icon(Icon::default().path("icons/settings.svg"))
            .description("App information")
            .group(SettingGroup::new().item(SettingItem::new(
                "Version",
                SettingField::<SharedString>::render(move |_, _, _| div().child(version.clone())),
            )))
    }

    fn update_agents(
        &mut self,
        change: Option<(clumsiesd::ProjectAgentAdapterKind, bool)>,
        cx: &mut Context<Self>,
    ) {
        if self.agents_busy {
            return;
        }
        self.agents_busy = true;
        // Refresh can finish a previously requested Codex installation.
        self.agents_mutating = true;
        self.agents_error = None;
        let work = cx.background_executor().spawn(async move {
            let changed = match change {
                Some((adapter, enabled)) => engine::configure_agent(adapter, enabled),
                None => engine::reconcile_codex_agent(),
            };
            (
                changed,
                engine::agent_settings(),
                engine::codex_plugin_status(),
            )
        });
        cx.spawn(async move |this, cx| {
            let (changed, settings, codex) = work.await;
            this.update(cx, |view, cx| {
                view.agents_busy = false;
                view.agents_mutating = false;
                view.codex_status = Some(codex);
                match settings {
                    Ok(settings) => view.agents = Some(settings),
                    Err(error) => view.agents_error = Some(error),
                }
                if let Err(error) = changed {
                    view.agents_error = Some(error);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn agents_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let mut group = SettingGroup::new().title("Agents on this machine")
            .description("Install once for all projects. Work folder bindings select which project's Memory each agent uses.");
        if let Some(settings) = &self.agents {
            for (index, setting) in settings.items.iter().enumerate() {
                let adapter = setting.adapter;
                let enabled = setting.enabled;
                let supported =
                    !cfg!(windows) || adapter == clumsiesd::ProjectAgentAdapterKind::Codex;
                let disabled = self.agents_busy || !supported || engine::isolated_connections();
                let label = match adapter {
                    clumsiesd::ProjectAgentAdapterKind::Codex => "Codex",
                    clumsiesd::ProjectAgentAdapterKind::ClaudeCode => "Claude Code",
                    clumsiesd::ProjectAgentAdapterKind::Opencode => "OpenCode",
                    clumsiesd::ProjectAgentAdapterKind::Dsh => "Dsh",
                    clumsiesd::ProjectAgentAdapterKind::Antigravity => "Antigravity",
                };
                let description = if engine::isolated_connections() {
                    "Agent installation is unavailable in this isolated development instance."
                } else if !supported {
                    "This integration is not yet available on Windows."
                } else if enabled && setting.installed {
                    if adapter == clumsiesd::ProjectAgentAdapterKind::Dsh {
                        "Enabled; MCP profile setup required."
                    } else {
                        "Installed for this machine."
                    }
                } else if enabled {
                    "Ready to install."
                } else {
                    ""
                };
                let mut description = description.to_owned();
                if adapter == clumsiesd::ProjectAgentAdapterKind::Codex
                    && !engine::isolated_connections()
                {
                    description = if !enabled {
                        "Disabled".into()
                    } else {
                        match &self.codex_status {
                            Some(Ok(status)) if !status.host_installed => {
                                "Will install when Codex is available".into()
                            }
                            Some(Ok(status)) if status.ready => {
                                "Plugin installed and enabled".into()
                            }
                            Some(Ok(status)) if status.plugin_installed => {
                                "Plugin needs repair".into()
                            }
                            Some(Ok(_)) => "Plugin not installed".into(),
                            Some(Err(error)) => error.clone(),
                            None => "Selected by default".into(),
                        }
                    };
                }
                if setting.configured && setting.legacy_repositories > 0 {
                    description.push_str(&format!(" {} old repository configuration(s) still need cleanup. Reconnect missing folders and retry.", setting.legacy_repositories));
                }
                let view = cx.entity().downgrade();
                let item = SettingItem::new(
                    label,
                    SettingField::<SharedString>::render(move |_, _, _| {
                        let view = view.clone();
                        Switch::new(("agent-enabled", index))
                            .checked(enabled)
                            .disabled(disabled)
                            .on_click(move |enabled, _, cx| {
                                view.update(cx, |view, cx| {
                                    view.update_agents(Some((adapter, *enabled)), cx)
                                })
                                .ok();
                            })
                    }),
                )
                .description(description);
                group = group.item(item);
            }
        }
        if self.agents.as_ref().is_some_and(|settings| {
            settings.items.iter().any(|setting| {
                setting.adapter == clumsiesd::ProjectAgentAdapterKind::Dsh && setting.enabled
            })
        }) {
            group = group.item(SettingItem::new(
                "",
                SettingField::<SharedString>::render(|_, _, cx| {
                    ui::message(
                        "Register the dsh MCP entry in your dsh profile.",
                        cx.theme().muted_foreground,
                    )
                }),
            ));
        }
        // macOS pageFeedback appears only on failure, with a retry action.
        if let Some(error) = &self.agents_error {
            let error = error.clone();
            let busy = self.agents_busy;
            let view = cx.entity().downgrade();
            group = group.item(SettingItem::new(
                "",
                SettingField::<SharedString>::render(move |_, _, cx| {
                    let view = view.clone();
                    v_flex()
                        .gap_2()
                        .child(ui::message(error.clone(), cx.theme().danger))
                        .child(
                            Button::new("retry-agents")
                                .label("Retry")
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    view.update(cx, |view, cx| view.update_agents(None, cx))
                                        .ok();
                                }),
                        )
                }),
            ));
        }
        SettingPage::new("Agents")
            .icon(Icon::default().path("icons/terminal.svg"))
            .group(group)
    }

    fn support_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let path = self
            .facts
            .log_dir
            .as_ref()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                crate::logging::path().and_then(|p| p.parent().map(std::path::Path::to_path_buf))
            });
        let logs = SettingItem::new(
            "Logs",
            SettingField::<SharedString>::render(move |_, _, _| {
                let path = path.clone();
                Button::new("show-logs")
                    .label("Open logs folder")
                    .disabled(path.is_none())
                    .on_click(move |_, _, cx| {
                        if let Some(path) = &path {
                            cx.reveal_path(path);
                        }
                    })
            }),
        );
        let view = cx.entity().downgrade();
        let busy = self.support_busy;
        let diagnostics = SettingItem::new(
            "Diagnostics",
            SettingField::<SharedString>::render(move |_, _, _| {
                let view = view.clone();
                Button::new("export-diagnostics")
                    .label("Export…")
                    .disabled(busy)
                    .loading(busy)
                    .on_click(move |_, _, cx| {
                        view.update(cx, |view, cx| view.export_diagnostics(cx)).ok();
                    })
            }),
        );
        let mut group = SettingGroup::new()
            .description("Use logs to help investigate a problem with Clumsies.")
            .item(logs)
            .item(diagnostics);
        if let Some(error) = &self.support_error {
            let error = error.clone();
            group = group.item(SettingItem::new(
                "Export",
                SettingField::<SharedString>::render(move |_, _, cx| {
                    ui::message(error.clone(), cx.theme().danger)
                }),
            ));
        }
        SettingPage::new("Support")
            .icon(Icon::default().path("icons/circle-question-mark.svg"))
            .description("Troubleshooting logs")
            .group(group)
    }

    fn export_diagnostics(&mut self, cx: &mut Context<Self>) {
        if self.support_busy {
            return;
        }
        self.support_busy = true;
        self.support_error = None;
        let picker =
            cx.prompt_for_new_path(std::path::Path::new(""), Some("Clumsies-Diagnostics.zip"));
        let log_dir = self.facts.log_dir.as_ref().map(std::path::PathBuf::from);
        cx.spawn(async move |this, cx| {
            let destination = match picker.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => {
                    this.update(cx, |view, cx| {
                        view.support_busy = false;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                result => {
                    this.update(cx, |view, cx| {
                        view.support_busy = false;
                        view.support_error =
                            Some(format!("Could not choose export destination: {result:?}"));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let target = destination.clone();
            let result = cx
                .background_executor()
                .spawn(async move { engine::export_diagnostics(&target, log_dir) })
                .await;
            this.update(cx, |view, cx| {
                view.support_busy = false;
                match result {
                    Ok(()) => cx.reveal_path(&destination),
                    Err(error) => view.support_error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn passwords(&self) -> bool {
        self.methods
            .as_ref()
            .is_ok_and(|methods| methods.password_enabled)
    }

    /// macOS disables the confirm button on the same conditions: a password of
    /// at least fifteen characters that matches, the current password when the
    /// account has one, and a username when the account has none.
    fn action_ready(&self, action: Action, cx: &App) -> bool {
        let text = |field: &Entity<InputState>| field.read(cx).value().to_string();
        if self.password_set() && text(&self.current_password).is_empty() {
            return false;
        }
        match action {
            Action::Connect => true,
            Action::Password => {
                let password = text(&self.password);
                password.chars().count() >= 15
                    && password == text(&self.confirmation)
                    && (self.has_username() || !text(&self.username).trim().is_empty())
            }
        }
    }

    fn begin(&mut self, action: Action, cx: &mut Context<Self>) {
        self.clear_form();
        self.error = None;
        self.notice = None;
        self.action = Some(action);
        cx.notify();
    }

    fn clear_form(&mut self) {
        self.action = None;
        self.clear_fields = true;
        self.confirm_close = false;
    }

    /// Runs one of the two account changes.
    ///
    /// Both answer with a fresh session, so both hand it to the daemon before
    /// the answer is reported: a password change signs every other session out,
    /// this one included, and connecting a provider re-issues this one.
    fn run(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let username = self.username.read(cx).value().trim().to_owned();
        let current = self.current_password.read(cx).value().to_string();
        let password = self.password.read(cx).value().to_string();
        let password_set = self.password_set();
        let google = self.methods.as_ref().is_ok_and(|methods| methods.google);
        self.busy = true;
        self.error = None;
        self.notice = None;
        cx.notify();

        let work = cx.background_executor().spawn(async move {
            let session = match action {
                Action::Password => engine::change_password(
                    (!username.is_empty()).then_some(username.as_str()),
                    password_set.then_some(current.as_str()),
                    &password,
                )?,
                Action::Connect => engine::bind_identity(password_set.then_some(current.as_str()))?,
            };
            let origin = engine::configured_server_url()
                .ok_or_else(|| "the daemon is not pointed at a Server".to_owned())?;
            engine::install_session(
                &origin,
                &session.access_token,
                session.refresh_token.as_deref(),
            )?;
            Ok::<String, String>(match action {
                Action::Password if password_set => "Password changed.".to_owned(),
                Action::Password => "Password set.".to_owned(),
                Action::Connect if google => "Google account connected.".to_owned(),
                Action::Connect => "Account connected.".to_owned(),
            })
        });
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |dialog, cx| {
                dialog.busy = false;
                dialog.clear_secrets = true;
                match result {
                    Ok(notice) => {
                        // The session belongs to the window, so the window
                        // re-reads whose it is; the pane re-reads what it shows.
                        dialog.credentials = engine::credentials();
                        dialog.clear_form();
                        dialog.notice = Some(notice);
                        dialog.error = None;
                        let _ = app.update(cx, |app, cx| app.reload_account(cx));
                    }
                    Err(error) => dialog.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_fields || self.clear_secrets {
            for field in [&self.current_password, &self.password, &self.confirmation] {
                field.update(cx, |state, cx| state.set_value("", window, cx));
            }
            if self.clear_fields {
                self.username
                    .update(cx, |state, cx| state.set_value("", window, cx));
            }
            self.clear_fields = false;
            self.clear_secrets = false;
        }
        div()
            .v_flex()
            .w_full()
            .h(px(modal::BODY))
            .child(self.surface(cx))
    }
}
