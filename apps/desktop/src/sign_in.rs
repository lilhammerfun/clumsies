//! Signing in to a Server, and setting one up when it has never been configured.
//!
//! A desktop client cannot ask a human to install a session, so it runs the
//! same authorization a browser would: a loopback listener for the callback,
//! PKCE, the system browser for the identity provider, and then the tokens to
//! the daemon -- which owns them, not this process. The macOS client does the
//! same in `AuthenticationClient` and `NativeServerSetupClient`; this is that
//! flow for the platforms that have no Keychain to hand a session to.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

/// Where the identity provider sends the browser back. The Server accepts any
/// loopback port with this path, so the port is free to be ephemeral.
const CALLBACK_PATH: &str = "/callback";
/// A person is expected to complete the browser step, so this is minutes.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    /// `setup_required` until the deployment has an organization.
    pub state: String,
    pub setup_code_configured: bool,
    pub oidc_configured: bool,
    /// A setup session the Server already holds, which carries the settings a
    /// previous attempt staged.
    pub session: Option<SetupSessionStatus>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupSessionStatus {
    pub configuration: Option<SetupConfiguration>,
}

/// First-run settings awaiting an owner, in the shape the Server stages them.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupConfiguration {
    pub org_name: String,
    pub default_project_name: String,
    pub allowed_email_domains: Vec<String>,
}

/// The CSRF proof a live setup session hands back, which is what the setup
/// calls then carry in a header.
#[derive(Clone, Deserialize)]
pub struct SetupSession {
    pub csrf_token: String,
}

#[derive(Clone, Deserialize)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

/// A Server that has never been configured asks to be set up before it will
/// admit a product login.
pub fn needs_setup(status: &SetupStatus) -> bool {
    status.state == "setup_required"
}

pub fn setup_status(origin: &str) -> Result<SetupStatus, String> {
    let client = client()?;
    let response = client
        .get(format!("{origin}/api/v1/setup"))
        .send()
        .map_err(|error| unreachable_server(origin, &error))?;
    read_json(response, "the Server's setup state")
}

/// Creates the first organization and owner. Returns the session that the
/// authorization which follows binds to that owner.
pub fn complete_setup(
    origin: &str,
    setup_code: &str,
    organization: &str,
    default_project: &str,
    allowed_email_domains: &[String],
) -> Result<Session, String> {
    let client = client()?;
    let session = client
        .post(format!("{origin}/api/v1/setup/sessions"))
        .json(&serde_json::json!({ "setupCode": setup_code }))
        .send()
        .map_err(|error| unreachable_server(origin, &error))?;
    // The setup session arrives as an HttpOnly cookie. This is the only place
    // that needs it, so it is passed along by hand rather than carrying a
    // cookie store through the whole client.
    let cookie = session
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .collect::<Vec<_>>()
        .join("; ");
    let session: SetupSession = read_json(session, "the setup session")?;

    let csrf = session.csrf_token;
    let response = client
        .put(format!("{origin}/api/v1/setup/configuration"))
        .header("cookie", &cookie)
        .header("x-csrf-token", &csrf)
        .json(&serde_json::json!({
            "orgName": organization,
            "defaultProjectName": default_project,
            "allowedEmailDomains": allowed_email_domains,
        }))
        .send()
        .map_err(|error| unreachable_server(origin, &error))?;
    ensure_success(response, "saving the Server configuration")?;

    let (verifier, challenge, state) = pkce();
    let redirect = loopback_redirect()?;
    let response = client
        .post(format!("{origin}/api/v1/setup/oidc-authorizations"))
        .header("cookie", &cookie)
        .header("x-csrf-token", &csrf)
        .json(&serde_json::json!({
            "redirect_uri": redirect.uri,
            "state": state,
            "code_challenge": challenge,
            "code_challenge_method": "S256",
        }))
        .send()
        .map_err(|error| unreachable_server(origin, &error))?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Authorization {
        authorization_url: String,
    }
    let authorization: Authorization = read_json(response, "the setup authorization")?;
    finish_authorization(
        &client,
        origin,
        redirect,
        &authorization.authorization_url,
        &verifier,
        &state,
    )
}

/// The ordinary path: the Server is configured, and this authorizes a user.
pub fn authenticate(origin: &str) -> Result<Session, String> {
    let client = client()?;
    let (verifier, challenge, state) = pkce();
    let redirect = loopback_redirect()?;
    let authorization = reqwest::Url::parse_with_params(
        &format!("{origin}/oauth2/authorization/oidc"),
        [
            ("client_kind", "desktop"),
            ("redirect_uri", redirect.uri.as_str()),
            ("state", state.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ],
    )
    .map_err(|error| format!("could not build the authorization request: {error}"))?
    .to_string();
    finish_authorization(&client, origin, redirect, &authorization, &verifier, &state)
}

/// Opens the browser, waits for the callback, and exchanges the code.
fn finish_authorization(
    client: &reqwest::blocking::Client,
    origin: &str,
    redirect: Callback,
    authorization: &str,
    verifier: &str,
    state: &str,
) -> Result<Session, String> {
    open_browser(authorization)?;
    let callback = redirect
        .listener
        .accept()
        .map_err(|error| format!("could not receive the authorization callback: {error}"))?;
    let code = read_callback(callback.0, state)?;

    let response = client
        .post(format!("{origin}/api/v1/auth/token"))
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": redirect.uri,
            "code_verifier": verifier,
        }))
        .send()
        .map_err(|error| unreachable_server(origin, &error))?;
    read_json(response, "the session")
}

/// One connection, one request line, one friendly answer.
fn read_callback(mut stream: std::net::TcpStream, expected_state: &str) -> Result<String, String> {
    stream
        .set_read_timeout(Some(CALLBACK_TIMEOUT))
        .map_err(|error| format!("could not wait for the callback: {error}"))?;
    let mut buffer = [0u8; 4096];
    let read = stream
        .read(&mut buffer)
        .map_err(|error| format!("could not read the callback: {error}"))?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| "the callback was not an HTTP request".to_owned())?;
    let target = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|error| format!("the callback was not a valid request: {error}"))?;
    let parameters: std::collections::HashMap<String, String> = target
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    let answer = if let Some(error) = parameters.get("error") {
        format!("The identity provider refused the request: {error}.")
    } else if parameters.get("state").map(String::as_str) != Some(expected_state) {
        "The callback did not match this request. Try again from Clumsies.".to_owned()
    } else {
        "Signed in. You can close this tab and return to Clumsies.".to_owned()
    };
    let _ = stream.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            answer.len(),
            answer
        )
        .as_bytes(),
    );

    if let Some(error) = parameters.get("error") {
        return Err(format!(
            "the identity provider refused the request: {error}"
        ));
    }
    if parameters.get("state").map(String::as_str) != Some(expected_state) {
        return Err("the callback did not match this request".to_owned());
    }
    parameters
        .get("code")
        .cloned()
        .ok_or_else(|| "the callback carried no authorization code".to_owned())
}

struct Callback {
    listener: TcpListener,
    uri: String,
}

fn loopback_redirect() -> Result<Callback, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("could not open a loopback port for the callback: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("could not read the callback port: {error}"))?
        .port();
    Ok(Callback {
        listener,
        uri: format!("http://127.0.0.1:{port}{CALLBACK_PATH}"),
    })
}

fn pkce() -> (String, String, String) {
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge, random_bytes_b64())
}

/// Two version-4 UUIDs: 32 bytes of randomness, which is inside the range PKCE
/// asks for, from a dependency the workspace already builds.
fn random_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes
}

fn random_bytes_b64() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes())
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("could not prepare an HTTP client: {error}"))
}

fn unreachable_server(origin: &str, error: &reqwest::Error) -> String {
    format!("could not reach {origin}: {error}")
}

fn read_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::blocking::Response,
    what: &str,
) -> Result<T, String> {
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("could not read {what}: {error}"))?;
    if !status.is_success() {
        return Err(describe_failure(status.as_u16(), &body));
    }
    serde_json::from_str(&body).map_err(|error| format!("could not read {what}: {error}"))
}

fn ensure_success(response: reqwest::blocking::Response, what: &str) -> Result<(), String> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().unwrap_or_default();
    Err(describe_failure(status.as_u16(), &body).replace("failed", what))
}

/// The Server answers failures with an envelope, and its message is the one
/// worth showing.
fn describe_failure(status: u16, body: &str) -> String {
    #[derive(Deserialize)]
    struct Envelope {
        error: Option<Detail>,
    }
    #[derive(Deserialize)]
    struct Detail {
        message: String,
    }
    serde_json::from_str::<Envelope>(body)
        .ok()
        .and_then(|envelope| envelope.error)
        .map(|detail| detail.message)
        .unwrap_or_else(|| format!("the Server answered HTTP {status}"))
}

/// The system browser, because the identity provider is the Server's
/// deployment and not ours to draw.
fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");

    command
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not open a browser for sign-in: {error}"))
}
