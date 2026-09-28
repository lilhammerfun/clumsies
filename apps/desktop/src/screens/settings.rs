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
//! What is not built, named here rather than left to be discovered: macOS's
//! Agents pane (this machine's agent integrations), the Organization's
//! administration panes (its name, members, Projects, access and audit), and
//! General's language and update controls, because this client has neither a
//! translation nor an updater.

use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Icon;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::setting::{
    SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
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
    pub server: Option<String>,
    pub daemon: Option<String>,
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
        Self {
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
        }
    }

    /// Whether an account change is open, and what to call its confirm button.
    pub fn footer_state(&self, cx: &App) -> FooterState {
        let Some(action) = self.action else {
            return FooterState::Done;
        };
        FooterState::Action {
            confirm: match action {
                Action::Password if self.password_set() => "Change password",
                Action::Password => "Set password",
                Action::Connect => "Continue",
            },
            enabled: !self.busy && self.action_ready(action, cx),
            busy: self.busy,
        }
    }

    pub fn cancel_action(&mut self, cx: &mut Context<Self>) {
        self.clear_form();
        cx.notify();
    }

    pub fn confirm_action(&mut self, cx: &mut Context<Self>) {
        if let Some(action) = self.action {
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
            .sidebar_width(px(190.))
            // The Account page is the first, which is where a reader who
            // opened Settings out of the account menu is going.
            .default_selected_index(SelectIndex {
                page_ix: 0,
                group_ix: None,
            })
            .page(self.account_page(cx))
            .page(self.general_page(cx))
            .page(self.support_page(cx))
            .into_any_element()
    }

    fn account_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let mut page = SettingPage::new("Account")
            .icon(Icon::default().path("icons/circle-user.svg"))
            .description("How you sign in, and what this account is connected to");

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
        let mut group = SettingGroup::new().title(title);
        if self.password_set() {
            let field = self.current_password.clone();
            group = group.item(SettingItem::new(
                "Current password",
                SettingField::<SharedString>::render(move |_options, _window, _cx| {
                    Input::new(&field)
                }),
            ));
        }
        if action == Action::Password {
            if !self.has_username() {
                let field = self.username.clone();
                group = group.item(SettingItem::new(
                    "Username",
                    SettingField::<SharedString>::render(move |_options, _window, _cx| {
                        Input::new(&field)
                    }),
                ));
            }
            let password = self.password.clone();
            group = group.item(SettingItem::new(
                "New password",
                SettingField::<SharedString>::render(move |_options, _window, _cx| {
                    Input::new(&password)
                }),
            ));
            let confirmation = self.confirmation.clone();
            group = group.item(
                SettingItem::new(
                    "Confirm new password",
                    SettingField::<SharedString>::render(move |_options, _window, _cx| {
                        Input::new(&confirmation)
                    }),
                )
                .description("At least 15 characters. Other sessions will be signed out."),
            );
        } else {
            group = group.description("Continue in your browser to connect your account.");
        }
        let _ = cx;
        group
    }

    /// Who this window is signed in as, what it is talking to, and the versions
    /// on this machine.
    fn general_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let mut account = SettingGroup::new().title("Account");
        for (label, value) in match &self.facts.account {
            Ok(account) => vec![
                ("Signed in as", account.user.identity_label().to_owned()),
                ("Signs in with", account.user.login_label().to_owned()),
                ("Role", account.user.role.clone()),
                ("Organization", account.organization.clone()),
            ],
            Err(error) => vec![("Account", error.clone())],
        } {
            account = account.item(SettingItem::new(
                label,
                SettingField::<SharedString>::render(move |_options, _window, cx| {
                    div()
                        .text_style(&ui::BODY)
                        .text_color(cx.theme().muted_foreground)
                        .child(value.clone())
                }),
            ));
        }
        let mut machine = SettingGroup::new().title("This machine");
        for (label, value) in [
            (
                "Server",
                self.facts
                    .server
                    .clone()
                    .unwrap_or_else(|| "not configured".to_owned()),
            ),
            (
                "Engine",
                match &self.facts.daemon {
                    Some(version) => format!("clumsiesd {version}"),
                    None => "not answering".to_owned(),
                },
            ),
            ("Client", format!("clumsies-desktop {}", self.facts.client)),
        ] {
            machine = machine.item(SettingItem::new(
                label,
                SettingField::<SharedString>::render(move |_options, _window, cx| {
                    div()
                        .text_style(&ui::BODY)
                        .text_color(cx.theme().muted_foreground)
                        .child(value.clone())
                }),
            ));
        }
        let _ = cx;
        SettingPage::new("General")
            .icon(Icon::default().path("icons/settings.svg"))
            .description("This account, this Server, and the versions on this machine")
            .group(account)
            .group(machine)
    }

    /// Where this machine keeps what a reader would be asked for.
    fn support_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let logs = self
            .facts
            .log_dir
            .clone()
            .unwrap_or_else(|| "the engine did not say".to_owned());
        let support = SettingGroup::new().title("Diagnostics").item(
            SettingItem::new(
                "Logs",
                SettingField::<SharedString>::render(move |_options, _window, cx| {
                    div()
                        .text_style(&ui::BODY)
                        .text_color(cx.theme().muted_foreground)
                        .child(logs.clone())
                }),
            )
            .description("What the engine and this client wrote, newest last."),
        );
        let _ = cx;
        SettingPage::new("Support")
            .icon(Icon::default().path("icons/life-buoy.svg"))
            .description("Where this machine keeps what it can tell you")
            .group(support)
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
                match result {
                    Ok(notice) => {
                        // The session belongs to the window, so the window
                        // re-reads whose it is; the pane re-reads what it shows.
                        dialog.credentials = engine::credentials();
                        dialog.action = None;
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

/// What the dialog's footer offers, which depends on whether an account change
/// is open. The window builds the footer from this, because the footer belongs
/// to the dialog surface rather than to the page inside it.
pub enum FooterState {
    /// Nothing is open: the only thing to do is leave.
    Done,
    /// A change is open: it can be cancelled or confirmed.
    Action {
        confirm: &'static str,
        enabled: bool,
        busy: bool,
    },
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .w_full()
            .h(px(modal::BODY))
            .child(self.surface(cx))
    }
}
