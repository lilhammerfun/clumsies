//! Settings: whose account this window is signed in as, what it signs in with,
//! and what this machine holds for it.
//!
//! Read from the macOS client's SettingsWindowView, SettingsNavigation and
//! AccountSecurityView. macOS opens a window with a sidebar — the account block
//! itself is the Account pane, then General, Agents, Support, and the
//! Organization's administration when the account's role grants it. This client
//! has one window, so Settings is a dialog over the work, which is the choice
//! the Project settings dialog already made; the panes are the same idea in the
//! same order.
//!
//! The Account pane is macOS's: what the account signs in with, and the two
//! things that can be done about it — a local password, and an identity provider
//! connected to it. Each opens in place rather than in a sheet, and each answers
//! with a fresh session, because changing a password signs every other session
//! out and connecting a provider re-issues this one; the new tokens go to the
//! daemon before the old ones stop working.
//!
//! What is not built, named here rather than left to be discovered: macOS's
//! Agents pane (this machine's agent integrations), the Organization's
//! administration panes (its name, members, Projects, access and audit), and
//! General's language and update controls, because this client has neither a
//! translation nor an updater.

use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, IconName, Sizable as _, WindowExt as _};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::engine::{self, Account, Credentials};
use crate::sign_in::LoginMethods;
use crate::ui::{self, Typography};

/// One page of Settings. macOS's `SettingsPane`, without the panes that need a
/// surface this client does not have yet.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Account,
    General,
    Support,
}

impl Pane {
    fn title(self) -> &'static str {
        match self {
            Pane::Account => "Account",
            Pane::General => "General",
            Pane::Support => "Support",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Pane::Account => "How you sign in, and what this account is connected to",
            Pane::General => "This account, this Server, and the versions on this machine",
            Pane::Support => "Where this machine keeps what it can tell you",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Pane::Account => "icons/circle-user.svg",
            Pane::General => "icons/settings.svg",
            Pane::Support => "icons/life-buoy.svg",
        }
    }
}

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
    pane: Pane,
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
            pane: Pane::Account,
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

    /// The pages, down the left of the dialog, the way macOS's Settings window
    /// lists them beside its content. The account block is the first of them,
    /// which is what macOS made of it: the identity is not a label above the
    /// panes, it is the pane that says how you sign in.
    fn pages(&self, cx: &mut Context<Self>) -> AnyElement {
        let (selected, hover, muted, foreground) = {
            let theme = cx.theme();
            (
                ui::selected_background(cx),
                theme.list_hover,
                theme.muted_foreground,
                theme.foreground,
            )
        };
        let (identity, organization) = self
            .facts
            .account
            .as_ref()
            .map(|account| {
                (
                    account.user.identity_label().to_owned(),
                    account.organization.clone(),
                )
            })
            .unwrap_or_else(|_| ("Not signed in".to_owned(), String::new()));

        let current = self.pane == Pane::Account;
        let tone = if current {
            cx.theme().accent_foreground
        } else {
            foreground
        };
        let account_row = div()
            .id("settings-pane-account")
            .h_flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded(px(ui::RADIUS))
            .child(
                Icon::new(IconName::CircleUser)
                    .with_size(px(20.))
                    .text_color(tone),
            )
            .child(
                div()
                    .v_flex()
                    .min_w(px(0.))
                    .child(
                        div()
                            .text_style(&ui::BODY)
                            .text_color(tone)
                            .truncate()
                            .child(identity),
                    )
                    .child(
                        div()
                            .text_style(&ui::CAPTION)
                            .text_color(muted)
                            .truncate()
                            .child(organization),
                    ),
            )
            .on_click(cx.listener(|dialog, _event, _window, cx| {
                dialog.pane = Pane::Account;
                cx.notify();
            }));
        let account_row = if current {
            account_row.bg(selected)
        } else {
            account_row.hover(move |style| style.bg(hover))
        };

        let rows = [Pane::General, Pane::Support].into_iter().map(|pane| {
            let current = pane == self.pane;
            let tone = if current {
                cx.theme().accent_foreground
            } else {
                foreground
            };
            let row = div()
                .id(("settings-pane", pane as usize))
                .h_flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded(px(ui::RADIUS))
                .text_style(&ui::BODY)
                .text_color(tone)
                .child(
                    Icon::default()
                        .path(pane.symbol())
                        .with_size(px(14.))
                        .text_color(tone),
                )
                .child(pane.title())
                .on_click(cx.listener(move |dialog, _event, _window, cx| {
                    dialog.pane = pane;
                    cx.notify();
                }));
            if current {
                row.bg(selected)
            } else {
                row.hover(move |style| style.bg(hover))
            }
        });

        div()
            .v_flex()
            .w(px(200.))
            .flex_shrink_0()
            .gap_1()
            .child(account_row)
            .children(rows)
            .child(div().flex_1())
            .child(
                div()
                    .px_2()
                    .text_style(&ui::CAPTION)
                    .text_color(muted)
                    .child(format!("Clumsies {}", self.facts.client)),
            )
            .into_any_element()
    }

    fn content(&self, cx: &mut Context<Self>) -> AnyElement {
        let (foreground, muted) = {
            let theme = cx.theme();
            (theme.foreground, theme.muted_foreground)
        };
        let mut rows: Vec<AnyElement> = vec![
            div()
                .text_style(&ui::BODY)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .child(self.pane.title())
                .into_any_element(),
            div()
                .text_style(&ui::CAPTION)
                .text_color(muted)
                .child(self.pane.subtitle())
                .into_any_element(),
        ];
        match self.pane {
            Pane::Account => rows.extend(self.account(cx)),
            Pane::General => rows.extend(self.general(cx)),
            Pane::Support => rows.extend(self.support(cx)),
        }
        div()
            .v_flex()
            .flex_1()
            .min_w(px(0.))
            .gap_3()
            .children(rows)
            .into_any_element()
    }

    /// What the account signs in with — macOS's Account pane: the username, the
    /// password, and the identity provider, each with the one thing that can be
    /// done about it.
    fn account(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = Vec::new();
        match &self.credentials {
            Ok(credentials) => {
                rows.push(entry(
                    "Username",
                    credentials
                        .username
                        .clone()
                        .unwrap_or_else(|| "Not set".to_owned()),
                    cx,
                ));
                let passwords = self
                    .methods
                    .as_ref()
                    .is_ok_and(|methods| methods.password_enabled);
                if passwords {
                    rows.push(entry_control(
                        "Password",
                        Button::new("settings-password")
                            .label(if credentials.password_set {
                                "Change password…"
                            } else {
                                "Set password…"
                            })
                            .disabled(self.busy || self.action.is_some())
                            .on_click(cx.listener(|dialog, _event, _window, cx| {
                                dialog.begin(Action::Password, cx);
                            })),
                        cx,
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
                    match &credentials.oidc_email {
                        Some(email) => rows.push(entry(title, email.clone(), cx)),
                        None => rows.push(entry_control(
                            title,
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
                                        .disabled(self.busy || self.action.is_some())
                                        .on_click(cx.listener(|dialog, _event, _window, cx| {
                                            dialog.begin(Action::Connect, cx);
                                        })),
                                ),
                            cx,
                        )),
                    }
                }
            }
            Err(error) => rows.push(entry("Account", error.clone(), cx)),
        }

        if let Some(action) = self.action {
            rows.extend(self.action_form(action, cx));
        }
        if let Some(error) = &self.error {
            rows.push(
                div()
                    .text_style(&ui::BODY)
                    .text_color(cx.theme().danger)
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        if let Some(notice) = &self.notice {
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

    /// The form an action opens in place, which is what macOS's Account pane
    /// does instead of a sheet: what the action needs, the sentence explaining
    /// it, and its own two buttons.
    fn action_form(&self, action: Action, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let password_set = self
            .credentials
            .as_ref()
            .is_ok_and(|credentials| credentials.password_set);
        let has_username = self
            .credentials
            .as_ref()
            .is_ok_and(|credentials| credentials.username.is_some());
        let mut rows: Vec<AnyElement> = vec![
            div()
                .pt(px(ui::SPACE_SM))
                .text_style(&ui::CAPTION)
                .font_weight(FontWeight::SEMIBOLD)
                .child(match action {
                    Action::Password if password_set => "Change password",
                    Action::Password => "Set password",
                    Action::Connect => "Verify your identity",
                })
                .into_any_element(),
        ];
        if password_set {
            rows.push(Input::new(&self.current_password).into_any_element());
        }
        if action == Action::Password {
            if !has_username {
                rows.push(Input::new(&self.username).into_any_element());
            }
            rows.push(Input::new(&self.password).into_any_element());
            rows.push(Input::new(&self.confirmation).into_any_element());
        }
        rows.push(
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child(match action {
                    Action::Password => {
                        "At least 15 characters. Other sessions will be signed out."
                    }
                    Action::Connect => "Continue in your browser to connect your account.",
                })
                .into_any_element(),
        );
        let confirm = match action {
            Action::Password if password_set => "Change password",
            Action::Password => "Set password",
            Action::Connect => "Continue",
        };
        rows.push(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .child(
                    Button::new("settings-cancel")
                        .label("Cancel")
                        .on_click(cx.listener(|dialog, _event, _window, cx| {
                            dialog.clear_form();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("settings-confirm")
                        .primary()
                        .label(confirm)
                        .disabled(self.busy || !self.action_ready(action, cx))
                        .on_click(cx.listener(move |dialog, _event, _window, cx| {
                            dialog.run(action, cx);
                        })),
                )
                .into_any_element(),
        );
        rows
    }

    /// macOS disables the confirm button on the same conditions: a password of
    /// at least fifteen characters that matches, the current password when the
    /// account has one, and a username when the account has none.
    fn action_ready(&self, action: Action, cx: &Context<Self>) -> bool {
        let password_set = self
            .credentials
            .as_ref()
            .is_ok_and(|credentials| credentials.password_set);
        let has_username = self
            .credentials
            .as_ref()
            .is_ok_and(|credentials| credentials.username.is_some());
        let text = |field: &Entity<InputState>| field.read(cx).value().to_string();
        if password_set && text(&self.current_password).is_empty() {
            return false;
        }
        match action {
            Action::Connect => true,
            Action::Password => {
                let password = text(&self.password);
                password.chars().count() >= 15
                    && password == text(&self.confirmation)
                    && (has_username || !text(&self.username).trim().is_empty())
            }
        }
    }

    /// Who this window is signed in as, what it is talking to, and the versions
    /// on this machine.
    fn general(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        match &self.facts.account {
            Ok(account) => {
                rows.push(entry(
                    "Signed in as",
                    account.user.identity_label().to_owned(),
                    cx,
                ));
                rows.push(entry(
                    "Signs in with",
                    account.user.login_label().to_owned(),
                    cx,
                ));
                rows.push(entry("Role", account.user.role.clone(), cx));
                rows.push(entry("Organization", account.organization.clone(), cx));
            }
            Err(error) => rows.push(entry("Account", error.clone(), cx)),
        }
        rows.push(entry(
            "Server",
            self.facts
                .server
                .clone()
                .unwrap_or_else(|| "not configured".to_owned()),
            cx,
        ));
        rows.push(entry(
            "Engine",
            match &self.facts.daemon {
                Some(version) => format!("clumsiesd {version}"),
                None => "not answering".to_owned(),
            },
            cx,
        ));
        rows.push(entry(
            "Client",
            format!("clumsies-desktop {}", self.facts.client),
            cx,
        ));
        rows
    }

    /// Where this machine keeps what a reader would be asked for.
    fn support(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![entry(
            "Logs",
            self.facts
                .log_dir
                .clone()
                .unwrap_or_else(|| "the engine did not say".to_owned()),
            cx,
        )];
        if self.facts.account.is_err() {
            rows.push(entry("Last refusal", "see the log above".to_owned(), cx));
        }
        rows
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
        let password_set = self
            .credentials
            .as_ref()
            .is_ok_and(|credentials| credentials.password_set);
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

/// One line of the dialog: what it is, and what it says.
fn entry(label: &str, value: String, cx: &App) -> AnyElement {
    entry_control(label, div().text_style(&ui::BODY).child(value), cx)
}

/// The same line with a control where the value would be.
fn entry_control(label: &str, control: impl IntoElement, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    div()
        .h_flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(120.))
                .flex_shrink_0()
                .text_style(&ui::CAPTION)
                .text_color(muted)
                .child(label.to_owned()),
        )
        .child(div().flex_1().min_w(px(0.)).child(control))
        .into_any_element()
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .w_full()
            .gap_4()
            .child(
                div()
                    .h_flex()
                    .items_start()
                    .gap_4()
                    .w_full()
                    .child(self.pages(cx))
                    .child(self.content(cx)),
            )
            .child(
                div().h_flex().justify_end().child(
                    Button::new("settings-done")
                        .primary()
                        .label("Done")
                        .on_click(|_event, window, cx| window.close_dialog(cx)),
                ),
            )
    }
}
