//! The sign-in screen: a form, in the shape the macOS client uses.
//!
//! Read from `apps/macos/Sources/Features/ServerAccess/NativeServerAccessView.swift`:
//! the same fields in the same order with the same words. What differs is the
//! metric and the form rules, which come from DESIGN.md: the required fields
//! are marked, the primary action stays disabled until they are filled, and a
//! failure is shown in the form rather than in a dialog.

use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::ui::{self, Typography};

/// macOS caps the form column at 410 points; a line that long is already hard
/// to read, so the cap travels with the design.
const FORM_WIDTH: f32 = 410.;

pub struct SignInScreen {
    pub server: Entity<InputState>,
    pub setup_code: Entity<InputState>,
    pub organization: Entity<InputState>,
    pub default_project: Entity<InputState>,
    pub allowed_domains: Entity<InputState>,
    /// The Server said it has never been configured, so it needs the extra
    /// four fields before it will admit anyone.
    pub shows_setup: bool,
    /// The Server reported that its deployment is missing these; the form says
    /// so instead of failing later.
    pub setup_code_configured: bool,
    pub oidc_configured: bool,
    pub busy: bool,
    /// Settings a previous setup attempt staged on the Server. The form offers
    /// them again rather than making the reader retype them, which is what the
    /// macOS form does with the same answer.
    pub staged: Option<StagedSetup>,
    /// A failure worth showing: a rejected value, an unreachable Server, a
    /// browser round trip that came back wrong.
    pub error: Option<String>,
    /// The step in progress, so a slow Server explains itself.
    pub stage: Option<String>,
}

impl SignInScreen {
    pub fn new(window: &mut Window, cx: &mut Context<DesktopApp>, server_url: &str) -> Self {
        let mut field = |cx: &mut Context<DesktopApp>, placeholder: &str, value: &str| {
            let placeholder = placeholder.to_owned();
            let value = value.to_owned();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        Self {
            server: field(cx, "https://clumsies.example.com", server_url),
            setup_code: field(cx, "Deployment setup code", ""),
            organization: field(cx, "Acme", ""),
            default_project: field(cx, "Default", "Default"),
            allowed_domains: field(cx, "example.com, subsidiary.example", ""),
            shows_setup: false,
            setup_code_configured: true,
            oidc_configured: true,
            staged: None,
            busy: false,
            error: None,
            stage: None,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.shows_setup {
            "Set up Clumsies Server"
        } else {
            "Sign in to Clumsies"
        }
    }

    pub fn subtitle(&self) -> &'static str {
        if self.shows_setup {
            "Create the first organization and owner. The browser finishes the sign-in."
        } else {
            "Connect to your Server, then continue in the system browser."
        }
    }

    /// Fills the setup fields from a staged configuration, once. A network
    /// answer arrives without a window and setting a field needs one, so the
    /// fill happens on the frame after the answer.
    pub fn apply_staged(&mut self, window: &mut Window, cx: &mut Context<DesktopApp>) {
        let Some(staged) = self.staged.take() else {
            return;
        };
        self.organization.update(cx, |state, cx| {
            state.set_value(staged.organization, window, cx)
        });
        self.default_project.update(cx, |state, cx| {
            state.set_value(staged.default_project, window, cx)
        });
        self.allowed_domains.update(cx, |state, cx| {
            state.set_value(staged.allowed_domains, window, cx)
        });
    }

    /// The values the form submits, for the flow in app.rs to use.
    pub fn values(&self, cx: &Context<DesktopApp>) -> FormValues {
        FormValues {
            server_origin: self.server.read(cx).value().to_string().trim().to_owned(),
            setup_code: self
                .setup_code
                .read(cx)
                .value()
                .to_string()
                .trim()
                .to_owned(),
            organization: self
                .organization
                .read(cx)
                .value()
                .to_string()
                .trim()
                .to_owned(),
            default_project: self
                .default_project
                .read(cx)
                .value()
                .to_string()
                .trim()
                .to_owned(),
            allowed_domains: domains(self.allowed_domains.read(cx).value().as_ref()),
        }
    }

    /// Windows' rule, which macOS does not state: a form that can be submitted
    /// invalid is a form that will be.
    pub fn ready(&self, cx: &Context<DesktopApp>) -> bool {
        let values = self.values(cx);
        if values.server_origin.is_empty() {
            return false;
        }
        if !self.shows_setup {
            return true;
        }
        !values.setup_code.is_empty()
            && !values.organization.is_empty()
            && !values.default_project.is_empty()
            && self.setup_code_configured
            && self.oidc_configured
    }

    pub fn render(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let busy = self.busy;
        let shows_setup = self.shows_setup;

        let mut form = div().v_flex().w(px(FORM_WIDTH)).gap_4().child(
            div()
                .v_flex()
                .gap_1()
                .child(div().text_style(&ui::SUBTITLE).child(self.title()))
                .child(
                    div()
                        .text_style(&ui::BODY)
                        .text_color(cx.theme().muted_foreground)
                        .child(self.subtitle()),
                ),
        );

        form = form.child(field(
            "Server address",
            Input::new(&self.server),
            "Remote Servers require HTTPS. HTTP is accepted only on the loopback address.",
            cx,
        ));

        if shows_setup {
            if !self.setup_code_configured {
                form = form.child(warning(
                    "Set CLUMSIES_SETUP_CODE in the Server deployment before continuing.",
                    cx,
                ));
            }
            if !self.oidc_configured {
                form = form.child(warning(
                    "Configure the Server's OIDC deployment settings before continuing.",
                    cx,
                ));
            }
            form = form
                .child(field("Setup code *", Input::new(&self.setup_code), "", cx))
                .child(field(
                    "Organization *",
                    Input::new(&self.organization),
                    "",
                    cx,
                ))
                .child(field(
                    "Default project *",
                    Input::new(&self.default_project),
                    "",
                    cx,
                ))
                .child(field(
                    "Allowed email domains (optional)",
                    Input::new(&self.allowed_domains),
                    "Separated by commas or spaces.",
                    cx,
                ));
        }

        let label = if shows_setup {
            "Save and continue in browser"
        } else {
            "Continue in browser"
        };
        let mut actions = div().v_flex().gap_2().items_start().child(
            Button::new("primary")
                .primary()
                .label(label)
                .disabled(busy || !self.ready(cx))
                .on_click(cx.listener(|this, _, _, cx| this.continue_from_form(cx))),
        );
        if shows_setup {
            actions = actions.child(
                Button::new("other-server")
                    .label("Use a different Server")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.choose_another_server(cx))),
            );
        }
        form = form.child(actions);

        if let Some(stage) = &self.stage {
            form = form.child(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child(stage.clone()),
            );
        }
        if let Some(error) = &self.error {
            // A form's failure belongs in the form. Windows says so explicitly:
            // contextual errors do not get a dialog.
            form = form.child(
                div()
                    .text_style(&ui::BODY)
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            );
        }

        div()
            .v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(form)
            .into_any_element()
    }
}

/// The first-run settings the Server already holds, ready for the form.
pub struct StagedSetup {
    pub organization: String,
    pub default_project: String,
    pub allowed_domains: String,
}

pub struct FormValues {
    pub server_origin: String,
    pub setup_code: String,
    pub organization: String,
    pub default_project: String,
    pub allowed_domains: Vec<String>,
}

/// macOS splits on commas, semicolons, whitespace and newlines and lowercases
/// the result; the same rule keeps two spellings of one domain from differing.
fn domains(input: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    input
        .split(|character: char| character == ',' || character == ';' || character.is_whitespace())
        .map(|domain| domain.trim().to_lowercase())
        .filter(|domain| !domain.is_empty())
        .filter(|domain| seen.insert(domain.clone()))
        .collect()
}

fn field(
    label: &str,
    control: impl IntoElement,
    help: &str,
    cx: &mut Context<DesktopApp>,
) -> AnyElement {
    let mut column = div().v_flex().gap_1().child(
        div()
            .text_style(&ui::CAPTION)
            .text_color(cx.theme().muted_foreground)
            .child(label.to_owned()),
    );
    column = column.child(control);
    if !help.is_empty() {
        column = column.child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child(help.to_owned()),
        );
    }
    column.into_any_element()
}

fn warning(message: &str, cx: &mut Context<DesktopApp>) -> AnyElement {
    div()
        .text_style(&ui::CAPTION)
        .text_color(cx.theme().warning)
        .child(message.to_owned())
        .into_any_element()
}
