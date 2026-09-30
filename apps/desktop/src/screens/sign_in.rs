//! The sign-in screen: the form the macOS client's `ServerAccess` page is.
//!
//! Read from `apps/macos/Sources/Features/ServerAccess/NativeServerAccessView.swift`
//! and `NativeServerAccessModel.swift`, in the state that page reached when it
//! gained invited password accounts: the brand mark over a title, the fields the
//! Server's own answer asks for — a local password when the deployment has
//! passwords, an invitation or a reset when a reader arrives with a one-time
//! credential, the browser when it has an identity provider — the four first-run
//! fields when the Server has never been configured, the Server address folded
//! away at the bottom, and the failure in the form. What differs is the metric
//! and the form rules, which come from DESIGN.md: the required fields are
//! marked, the primary action stays disabled until they are filled or the Server
//! has been asked, and a failure is shown in the form rather than in a dialog.
//! Metrics are the library's, which is what DESIGN.md's platform rule asks for
//! on Linux: the layout is macOS's — buttons that fill the column, a 20-point
//! title beside the mark — while heights, radii and type steps are the ones the
//! component library already draws.

use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::*;

use crate::app::DesktopApp;
use crate::sign_in::LoginMethods;
use crate::ui::{self, Typography};

/// macOS caps the page at 320 points and pads it 28 above and below; a line that
/// long is already hard to read, so the cap travels with the design.
const FORM_WIDTH: f32 = 320.;

/// Which of the local form's jobs is on screen. macOS's `LocalAction`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LocalAction {
    /// A username and a password.
    SignIn,
    /// A one-time credential, a username, and the password being chosen.
    Invitation,
    /// A one-time credential and the password being chosen.
    Reset,
}

impl LocalAction {
    /// The primary button's word, which is what the action is called.
    fn action(self) -> &'static str {
        match self {
            LocalAction::SignIn => "Sign in",
            LocalAction::Invitation => "Accept invitation",
            LocalAction::Reset => "Reset password",
        }
    }

    /// What the reader is holding, when they arrived with something.
    fn credential_help(self) -> Option<&'static str> {
        match self {
            LocalAction::SignIn => None,
            LocalAction::Invitation => Some("Paste the invitation from your administrator."),
            LocalAction::Reset => Some("Ask your administrator for a password reset credential."),
        }
    }
}

pub struct SignInScreen {
    pub server: Entity<InputState>,
    pub setup_code: Entity<InputState>,
    pub organization: Entity<InputState>,
    pub default_project: Entity<InputState>,
    pub allowed_domains: Entity<InputState>,
    pub username: Entity<InputState>,
    pub password: Entity<InputState>,
    pub confirm: Entity<InputState>,
    pub credential: Entity<InputState>,
    /// What the Server at the address says it offers, once it has been asked.
    /// The form is built from this: the browser button only when there is an
    /// identity provider, the local fields only when there are passwords.
    pub methods: Option<LoginMethods>,
    pub local_action: LocalAction,
    /// The address the Server has already been asked about. Entering it again is
    /// not worth another round trip, which is macOS's `serverReady`.
    pub checked: Option<String>,
    /// The Server address is folded away: it is set once and then read.
    pub server_expanded: bool,
    /// The Server said it has never been configured, so it needs the extra four
    /// fields before it will admit anyone.
    pub shows_setup: bool,
    /// Whether the first run is creating a local owner, which macOS offers when
    /// the deployment has passwords and only asks when it also has a provider.
    pub setup_with_password: bool,
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
    /// Whether the form has already asked the Server what it offers. macOS asks
    /// when the page appears; this is the same once, and only once.
    pub asked: bool,
}

impl SignInScreen {
    pub fn new(window: &mut Window, cx: &mut Context<DesktopApp>, server_url: &str) -> Self {
        let mut field =
            |cx: &mut Context<DesktopApp>, placeholder: &str, value: &str, secret: bool| {
                let placeholder = placeholder.to_owned();
                let value = value.to_owned();
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                        .masked(secret)
                })
            };
        Self {
            server: field(cx, "https://clumsies.example.com", server_url, false),
            setup_code: field(cx, "Deployment setup code", "", true),
            organization: field(cx, "Acme", "", false),
            default_project: field(cx, "Default", "Default", false),
            allowed_domains: field(cx, "example.com, subsidiary.example", "", false),
            username: field(cx, "Username", "", false),
            password: field(cx, "Password", "", true),
            confirm: field(cx, "Confirm password", "", true),
            credential: field(cx, "One-time credential", "", true),
            methods: None,
            local_action: LocalAction::SignIn,
            checked: None,
            server_expanded: false,
            shows_setup: false,
            setup_with_password: false,
            setup_code_configured: true,
            oidc_configured: true,
            staged: None,
            busy: false,
            error: None,
            stage: None,
            asked: false,
        }
    }

    /// Whether the form should ask the Server about itself on this frame. macOS
    /// does it in the page's own `task`, which runs once when the page appears.
    pub fn should_ask(&self, cx: &Context<DesktopApp>) -> bool {
        !self.asked && !self.busy && self.methods.is_none() && !self.origin(cx).is_empty()
    }

    pub fn title(&self) -> &'static str {
        if self.shows_setup {
            "Set up Clumsies Server"
        } else {
            "Sign in to Clumsies"
        }
    }

    /// Whether the Server in the address field has been asked already and is
    /// still the one being asked about.
    pub fn server_ready(&self, cx: &Context<DesktopApp>) -> bool {
        self.checked.as_deref() == Some(self.origin(cx).as_str())
    }

    /// Whether this deployment offers passwords, which is what the local fields
    /// and the two links are for.
    fn passwords(&self) -> bool {
        self.methods
            .as_ref()
            .is_some_and(|methods| methods.password_enabled)
    }

    /// Whether this deployment has an identity provider, which is what the
    /// browser button is for.
    fn provider(&self) -> bool {
        self.methods
            .as_ref()
            .is_some_and(|methods| methods.oidc_enabled)
    }

    /// Whether the address field holds something worth asking about.
    fn has_origin(&self, cx: &Context<DesktopApp>) -> bool {
        !self.origin(cx).is_empty()
    }

    fn origin(&self, cx: &Context<DesktopApp>) -> String {
        self.server.read(cx).value().trim().to_owned()
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

    /// Clears what a different Server, or a different action, makes stale —
    /// macOS clears the same fields in the same two places.
    pub fn forget_secrets(&mut self, window: &mut Window, cx: &mut Context<DesktopApp>) {
        for field in [&self.password, &self.confirm, &self.credential] {
            field.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.error = None;
    }

    /// The values the form submits, for the flow in app.rs to use.
    pub fn values(&self, cx: &Context<DesktopApp>) -> FormValues {
        let text = |field: &Entity<InputState>, cx: &Context<DesktopApp>| {
            field.read(cx).value().trim().to_owned()
        };
        FormValues {
            server_origin: self.origin(cx),
            setup_code: text(&self.setup_code, cx),
            organization: text(&self.organization, cx),
            default_project: text(&self.default_project, cx),
            allowed_domains: domains(self.allowed_domains.read(cx).value().as_ref()),
            username: text(&self.username, cx),
            password: self.password.read(cx).value().to_string(),
            confirm: self.confirm.read(cx).value().to_string(),
            credential: text(&self.credential, cx),
        }
    }

    /// What the primary button does, which is the one action the form is for.
    pub fn primary(&self) -> Primary {
        if self.shows_setup {
            if self.setup_with_password {
                Primary::CreateOwner
            } else {
                Primary::BrowserSetup
            }
        } else {
            match self.local_action {
                LocalAction::SignIn => Primary::PasswordSignIn,
                LocalAction::Invitation => Primary::AcceptInvitation,
                LocalAction::Reset => Primary::ResetPassword,
            }
        }
    }

    /// Windows' rule, which macOS does not state: a form that can be submitted
    /// invalid is a form that will be. The Server has to have answered first,
    /// because until it has, the form does not know which fields it wants.
    pub fn ready(&self, cx: &Context<DesktopApp>) -> bool {
        let values = self.values(cx);
        if values.server_origin.is_empty() || !self.server_ready(cx) {
            return false;
        }
        if self.shows_setup {
            return !values.setup_code.is_empty()
                && !values.organization.is_empty()
                && !values.default_project.is_empty()
                && self.setup_code_configured
                && (self.setup_with_password || self.oidc_configured);
        }
        match self.primary() {
            Primary::PasswordSignIn => !values.password.is_empty(),
            Primary::AcceptInvitation | Primary::ResetPassword => {
                !values.password.is_empty() && !values.credential.is_empty()
            }
            _ => true,
        }
    }

    pub fn render(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let mut form = div()
            .v_flex()
            .w(px(FORM_WIDTH))
            .gap_3()
            .child(self.header(cx));

        if self.shows_setup {
            form = form.children(self.setup_fields(cx));
            if self.passwords() && self.oidc_configured {
                form = form.child(self.setup_choice(cx));
            }
            if self.setup_with_password {
                form = form.children(self.local_fields(cx));
            }
            form = form.child(self.primary_button(cx));
        } else {
            if self.passwords() {
                form = form.children(self.local_fields(cx));
                form = form.child(self.primary_button(cx));
            }
            if self.local_action == LocalAction::SignIn && self.provider() {
                if self.passwords() {
                    form = form.child(separator(cx));
                }
                form = form.child(self.browser_button(cx));
            }
            if self.passwords() {
                form = form.child(self.local_links(cx));
            }
        }

        form = form.child(self.server_row(cx));

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

    /// The mark, then what the page is for. macOS puts the two on one line,
    /// six points apart, with the title at 20 points semibold — a step below the
    /// page titles elsewhere, because it names a form rather than a destination.
    fn header(&self, _cx: &App) -> AnyElement {
        div()
            .h_flex()
            .items_center()
            .gap_2()
            .pb_1()
            // The mark is drawn rather than typed, which is what macOS's
            // `BrandLogoView` is: the one place the product shows its face
            // before it knows who is signing in.
            .child(gpui_kit::img("brand/brand-mark.png").w(px(40.)).h(px(40.)))
            .child(
                div()
                    .text_style(&ui::SUBTITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title().to_owned()),
            )
            .into_any_element()
    }

    /// The first-run fields: the code that proves the deployment, and what the
    /// Server is being set up as.
    fn setup_fields(&self, cx: &mut Context<DesktopApp>) -> Vec<AnyElement> {
        let mut fields = Vec::new();
        if !self.setup_code_configured {
            fields.push(warning(
                "Set CLUMSIES_SETUP_CODE in the Server deployment before continuing.",
                cx,
            ));
        }
        if !self.oidc_configured && !self.setup_with_password {
            fields.push(warning(
                "Configure the Server's OIDC deployment settings before continuing.",
                cx,
            ));
        }
        fields.push(field(
            "Setup code *",
            Input::new(&self.setup_code),
            "From the Server deployment.",
            cx,
        ));
        fields.push(field(
            "Organization *",
            Input::new(&self.organization),
            "",
            cx,
        ));
        fields.push(field(
            "Default project *",
            Input::new(&self.default_project),
            "",
            cx,
        ));
        fields.push(field(
            "Allowed email domains (optional)",
            Input::new(&self.allowed_domains),
            "Separated by commas or spaces.",
            cx,
        ));
        fields
    }

    /// macOS's switch between creating a local owner and handing the first run
    /// to the identity provider.
    fn setup_choice(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let on = self.setup_with_password;
        div()
            .h_flex()
            .items_center()
            .gap_2()
            .child(
                Checkbox::new("setup-with-password")
                    .checked(on)
                    .label("Set up with username and password")
                    .on_click(cx.listener(|this, checked: &bool, _window, cx| {
                        this.sign_in_mut().setup_with_password = *checked;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// The local fields, which are the same three the Server asks for whether
    /// the reader is signing in, accepting an invitation, resetting a password,
    /// or creating the owner — macOS keeps one stack for all of them, and names
    /// each field in its placeholder rather than on a label above it. The
    /// first-run stack is the one that carries labels, and it does here too.
    fn local_fields(&self, cx: &mut Context<DesktopApp>) -> Vec<AnyElement> {
        let setup = self.shows_setup;
        let action = self.local_action;
        let mut fields = Vec::new();
        if !setup && action != LocalAction::SignIn {
            if let Some(help) = action.credential_help() {
                fields.push(
                    div()
                        .text_style(&ui::CAPTION)
                        .text_color(cx.theme().muted_foreground)
                        .child(help)
                        .into_any_element(),
                );
            }
            fields.push(Input::new(&self.credential).into_any_element());
        }
        if setup || action != LocalAction::Reset {
            fields.push(Input::new(&self.username).into_any_element());
        }
        fields.push(Input::new(&self.password).into_any_element());
        if setup || action != LocalAction::SignIn {
            fields.push(Input::new(&self.confirm).into_any_element());
            fields.push(
                div()
                    .text_style(&ui::CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child("Use at least 15 characters. Usernames use 3–32 letters, digits, dots, underscores or hyphens.")
                    .into_any_element(),
            );
        }
        fields
    }

    /// The one action the form's primary button takes, and its word for it.
    fn primary_button(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let label = match self.primary() {
            Primary::CreateOwner => "Create owner",
            Primary::BrowserSetup => "Continue with identity provider",
            Primary::PasswordSignIn | Primary::AcceptInvitation | Primary::ResetPassword => {
                self.local_action.action()
            }
        };
        let button = Button::new("primary")
            .primary()
            .label(label)
            .w_full()
            .disabled(self.busy || !self.ready(cx));
        let button = match self.primary() {
            Primary::BrowserSetup => {
                button.on_click(cx.listener(|this, _, _, cx| this.continue_in_browser(cx)))
            }
            Primary::CreateOwner => {
                button.on_click(cx.listener(|this, _, _, cx| this.complete_setup(cx)))
            }
            _ => button.on_click(cx.listener(|this, _, _, cx| this.sign_in_with_password(cx))),
        };
        div()
            .v_flex()
            .gap_2()
            .items_start()
            .child(button)
            .into_any_element()
    }

    /// The browser, which is the whole of the sign-in when the deployment has no
    /// passwords. Google's mark travels inside the button when that is the
    /// provider, as it does in macOS.
    fn browser_button(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let google = self.methods.as_ref().is_some_and(|methods| methods.google);
        let label = if google {
            "Sign in with Google"
        } else {
            "Sign in with identity provider"
        };
        // The mark is an image rather than an icon, so it is a child of the
        // button rather than its icon slot: macOS draws the two the same way
        // round — the G, then the label, centred in a button that fills the
        // column.
        let mut button = Button::new("browser-sign-in")
            .label(label)
            .w_full()
            .disabled(self.busy || !self.server_ready(cx))
            .on_click(cx.listener(|this, _, _, cx| this.continue_in_browser(cx)));
        if google {
            button = button.child(gpui_kit::img("brand/google-g.png").w(px(20.)).h(px(20.)));
        }
        button.into_any_element()
    }

    /// macOS's two links: the way into an invitation, and the way to a reset.
    /// They carry the accent, which is what `Color.accentColor` is in macOS:
    /// the library's text button draws the foreground colour, so the colour is
    /// asked for rather than inherited.
    fn local_links(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let action = self.local_action;
        let other = if action == LocalAction::SignIn {
            "Accept invitation"
        } else {
            "Back to sign in"
        };
        let link = cx.theme().link;
        let mut row = div()
            .h_flex()
            .items_center()
            .justify_between()
            .text_style(&ui::CAPTION)
            .child(
                Button::new("local-action")
                    .text()
                    .label(other)
                    .text_color(link)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.switch_local_action(
                            if action == LocalAction::SignIn {
                                LocalAction::Invitation
                            } else {
                                LocalAction::SignIn
                            },
                            window,
                            cx,
                        );
                    })),
            );
        if action == LocalAction::SignIn {
            row = row.child(
                Button::new("forgot-password")
                    .text()
                    .label("Forgot password?")
                    .text_color(link)
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_local_action(LocalAction::Reset, window, cx);
                    })),
            );
        }
        row.into_any_element()
    }

    /// The Server address, folded away: it is set once and then read, and macOS
    /// keeps it at the bottom of the page for the same reason.
    fn server_row(&self, cx: &mut Context<DesktopApp>) -> AnyElement {
        let origin = self.origin(cx);
        let expanded = self.server_expanded;
        let mut column = div().v_flex().gap_2().pt_2().child(
            div()
                .id("server-disclosure")
                .h_flex()
                .items_center()
                .gap_2()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child(
                    Icon::default()
                        .path(if expanded {
                            "icons/chevron-down.svg"
                        } else {
                            "icons/chevron-right.svg"
                        })
                        .with_size(px(12.))
                        .text_color(cx.theme().muted_foreground),
                )
                .child("Server address")
                .child(div().flex_1())
                .child(div().max_w(px(150.)).truncate().child(origin.clone()))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    let expanded = this.sign_in_mut().server_expanded;
                    this.sign_in_mut().server_expanded = !expanded;
                    cx.notify();
                })),
        );
        if expanded {
            column = column.child(
                div()
                    .h_flex()
                    .items_end()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.server)))
                    .child(
                        Button::new("connect")
                            .primary()
                            .label("Connect")
                            .w(px(88.))
                            .disabled(self.busy || self.server_ready(cx) || !self.has_origin(cx))
                            .on_click(cx.listener(|this, _, _, cx| this.connect_to_server(cx))),
                    ),
            );
        }
        column.into_any_element()
    }
}

/// The one action the form's primary button takes. macOS decides the same thing
/// from its own state, and the wording of the button is the state's name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Primary {
    /// A local owner is being created on a Server that has never been set up.
    CreateOwner,
    /// The first run goes to the identity provider.
    BrowserSetup,
    /// A username and password against an account that exists.
    PasswordSignIn,
    /// A one-time credential plus the password being chosen.
    AcceptInvitation,
    /// A one-time credential plus the password being chosen again.
    ResetPassword,
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
    pub username: String,
    pub password: String,
    pub confirm: String,
    pub credential: String,
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

/// macOS's rule between the two ways in, which it draws only when both exist.
fn separator(cx: &App) -> AnyElement {
    let rule = cx.theme().border;
    div()
        .h_flex()
        .items_center()
        .gap_3()
        .py_1()
        .child(div().flex_1().h(px(1.)).bg(rule))
        .child(
            div()
                .text_style(&ui::CAPTION)
                .text_color(cx.theme().muted_foreground)
                .child("or"),
        )
        .child(div().flex_1().h(px(1.)).bg(rule))
        .into_any_element()
}
