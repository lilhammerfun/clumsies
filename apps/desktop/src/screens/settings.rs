//! Settings: whose account this window is signed in as, and what this machine
//! holds for it.
//!
//! Read from the macOS client's SettingsWindowView and SettingsNavigation. macOS
//! opens a window with a sidebar — the account, "Login methods…", then General,
//! Agents, Support, and the Organization's administration when the account's
//! role grants it. This client has one window, so Settings is a dialog over the
//! work, which is the choice the Project settings dialog already made; the panes
//! are the same idea in the same order.
//!
//! What is not built, named here rather than left to be discovered: the
//! Organization's administration panes (its name, members, Projects, access and
//! audit) wait on an administration surface this client does not have yet, and
//! General carries no language or update controls, because this client has
//! neither a translation nor an updater — the panes say what this machine is
//! talking to instead, which is what a reader opens them to find out.

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::*;
use gpui_kit::component::{Icon, Sizable as _, WindowExt as _};
use gpui_kit::*;

use crate::engine::Account;
use crate::ui::{self, Typography};

/// One page of Settings. macOS's `SettingsPane`, without the panes that need a
/// surface this client does not have yet.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    General,
    Support,
}

impl Pane {
    const ALL: [Pane; 2] = [Pane::General, Pane::Support];

    fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Support => "Support",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Pane::General => "This account, this Server, and the versions on this machine",
            Pane::Support => "Where this machine keeps what it can tell you",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Pane::General => "icons/settings.svg",
            Pane::Support => "icons/life-buoy.svg",
        }
    }
}

/// Everything the dialog shows. It is read before the dialog opens, because
/// every line is a socket call and a dialog that arrives while it is still
/// loading flickers — the same rule the Project settings dialog follows.
pub struct Facts {
    pub account: Result<Account, String>,
    pub server: Option<String>,
    pub daemon: Option<String>,
    pub log_dir: Option<String>,
    pub client: &'static str,
}

pub struct SettingsDialog {
    facts: Facts,
    pane: Pane,
}

impl SettingsDialog {
    pub fn new(facts: Facts) -> Self {
        Self {
            facts,
            pane: Pane::General,
        }
    }

    /// The pages, down the left of the dialog, the way macOS's Settings window
    /// lists them beside its content.
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
        let rows = Pane::ALL.iter().map(|pane| {
            let pane = *pane;
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
            .w(px(168.))
            .flex_shrink_0()
            .gap_1()
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

    /// Who this window is signed in as, what it is talking to, and the versions
    /// on this machine.
    fn general(&self, cx: &App) -> Vec<AnyElement> {
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

    /// Where this machine keeps what a reader would be asked for: the engine's
    /// logs, and the account the Server sees.
    fn support(&self, cx: &App) -> Vec<AnyElement> {
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
}

/// One line of the dialog: what it is, and what it says.
fn entry(label: &str, value: String, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    div()
        .h_flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .w(px(96.))
                .flex_shrink_0()
                .text_style(&ui::CAPTION)
                .text_color(muted)
                .child(label.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_style(&ui::BODY)
                .child(value),
        )
        .into_any_element()
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div()
            .h_flex()
            .items_start()
            .gap_4()
            .w_full()
            .child(self.pages(cx))
            .child(self.content(cx));
        div().v_flex().w_full().gap_4().child(body).child(
            div().h_flex().justify_end().child(
                Button::new("settings-done")
                    .primary()
                    .label("Done")
                    .on_click(|_event, window, cx| window.close_dialog(cx)),
            ),
        )
    }
}
