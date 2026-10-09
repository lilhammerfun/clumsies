//! Human CLI for the resident daemon and version-checked Server Review workflow.
#![deny(
    missing_docs,
    clippy::missing_docs_in_private_items,
    clippy::missing_errors_doc
)]

mod review;

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clumsiesd::*;
use serde_json::{Value, json};

/// Human CLI; JSON output preserves server pagination and coordination evidence.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Action to perform.
    #[command(subcommand)]
    command: Command,
}

/// Supported product operations; no generic daemon or arbitrary HTTP escape hatch.
#[derive(Subcommand)]
enum Command {
    /// Show daemon, authentication, sync, and model readiness without starting it.
    Status {
        /// Include retrieval model and index readiness for this Project.
        #[arg(long)]
        project: Option<String>,
    },
    /// Manage the standalone user daemon (macOS uses the App).
    Daemon {
        /// Lifecycle action.
        #[command(subcommand)]
        action: DaemonCommand,
    },
    /// Sign in with a password or the deployment's browser identity provider.
    Login {
        /// Server origin; HTTPS except localhost development servers.
        #[arg(long)]
        server: String,
        /// Password account; omitted for browser sign-in.
        #[arg(long)]
        username: Option<String>,
        /// Read a password from stdin instead of a hidden terminal prompt.
        #[arg(long, requires = "username")]
        password_stdin: bool,
        /// Print the browser URL and wait for a manually opened callback.
        #[arg(long, conflicts_with = "username")]
        no_browser: bool,
    },
    /// Accept a password invitation or redeem a password-reset credential.
    Redeem {
        /// Server origin.
        #[arg(long)]
        server: String,
        /// Invited account username; omitted for password reset.
        #[arg(
            long,
            required_unless_present = "reset_password",
            conflicts_with = "reset_password"
        )]
        username: Option<String>,
        /// Redeem a password reset instead of an invitation.
        #[arg(long)]
        reset_password: bool,
        /// Read a JSON object with token and password from stdin, without echoing secrets.
        #[arg(long)]
        stdin: bool,
    },
    /// Revoke the session if reachable, then forget local credentials.
    Logout,
    /// Access projects and bind local workspaces.
    Project {
        /// Project action.
        #[command(subcommand)]
        action: ProjectCommand,
    },
    /// Configure a supported Agent host using daemon-owned adapters.
    Agent {
        /// Adapter action.
        #[command(subcommand)]
        action: AgentCommand,
    },
    /// Inspect and synchronize local proposals.
    Draft {
        /// Draft action.
        #[command(subcommand)]
        action: DraftCommand,
    },
    /// Request, inspect, discuss, decide, reconcile, and publish Reviews.
    Review {
        /// Review action.
        #[command(subcommand)]
        action: review::ReviewCommand,
    },
}

/// Explicit standalone lifecycle actions.
#[derive(Subcommand)]
enum DaemonCommand {
    /// Start on demand or reuse the existing daemon.
    Start,
    /// Stop without deleting credentials, bindings, or drafts.
    Stop,
    /// Stop and start the installed daemon.
    Restart,
}

/// Authorized project access; binding does not grant membership.
#[derive(Subcommand)]
enum ProjectCommand {
    /// List accessible projects, retaining server pagination metadata.
    List,
    /// Inspect a project you already have permission to access.
    Show {
        /// Server project identifier.
        id: String,
    },
    /// Create a project using your Server permissions.
    Create {
        /// Project name.
        name: String,
    },
    /// Select an accessible project for client operations.
    Join {
        /// Project identifier; membership must already be granted by an administrator.
        id: String,
    },
    /// Bind an existing directory to an accessible project.
    Bind {
        /// Project identifier.
        id: String,
        /// Workspace directory.
        path: PathBuf,
        /// Required prior binding revision when replacing a binding.
        #[arg(long)]
        revision: Option<i64>,
    },
    /// Remove an inspected binding without deleting local work.
    Unbind {
        /// Workspace root from the binding record.
        path: PathBuf,
        /// Revision from the inspected binding.
        #[arg(long)]
        revision: i64,
    },
    /// List the project's local directory bindings.
    Bindings {
        /// Project identifier.
        id: String,
    },
    /// Resolve the current directory's binding, including worktree inheritance.
    Current,
}

/// Global Agent installation preferences, persisted by the daemon.
#[derive(Subcommand)]
enum AgentCommand {
    /// Inspect global adapter settings.
    List,
    /// Enable and install a supported host integration.
    Enable {
        /// codex, claude-code, opencode, dsh, or antigravity.
        host: String,
        /// Installed Codex executable; defaults to codex on PATH.
        #[arg(long)]
        host_binary: Option<PathBuf>,
    },
    /// Remove daemon-managed integration files without overwriting user changes.
    Disable {
        /// Supported host name.
        host: String,
        /// Installed Codex executable used to remove the managed plugin.
        #[arg(long)]
        host_binary: Option<PathBuf>,
    },
}

/// Local draft inspection and bounded upload barrier.
#[derive(Subcommand)]
enum DraftCommand {
    /// Retry failed or interrupted upload operations for a Project.
    Retry {
        /// Project identifier.
        project: String,
    },
    /// List local drafts and continuation cursor.
    List {
        /// Opaque cursor returned by an earlier page.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Inspect operations, synchronization errors, and remote identity.
    Show {
        /// Local draft identifier.
        id: String,
    },
    /// Wait for all pending operations of a draft to reach the Server.
    Sync {
        /// Local draft identifier.
        id: String,
    },
    /// Compute reconciliation evidence for an uploaded draft.
    Plan {
        /// Local draft identifier.
        id: String,
    },
    /// Apply an inspected candidate with optional author-edited resolved state.
    Rebase {
        /// Local draft identifier.
        id: String,
        /// Candidate identifier from plan output.
        #[arg(long)]
        candidate: String,
        /// Exact server draft version from the candidate.
        #[arg(long)]
        version: i64,
        /// Expected reference from the candidate; use ref-none for an empty reference.
        #[arg(long)]
        reference: String,
        /// JSON ReconciliationResourceState for a conflicting candidate.
        #[arg(long)]
        resolved: Option<PathBuf>,
    },
}

/// Parses commands, reports errors on stderr, and uses a nonzero failure status.
fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("clumsies: {error}");
        if matches!(error.downcast_ref::<DaemonError>(), Some(DaemonError::Remote(error)) if error.code == "missing_session")
        {
            eprintln!("Run clumsies login again; local drafts and bindings are retained.");
        }
        std::process::exit(1);
    }
}

/// Performs one human operation without retaining any client credentials.
///
/// # Errors
/// Propagates validation, local transport, authentication, and Server failures.
fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::Status { project } = cli.command {
        let config = DaemonConfig::from_env()?;
        let client = DaemonIpcClient::new(config.mach_service_name)
            .with_timeout(std::time::Duration::from_secs(2));
        let sync = if let Some(project_id) = &project {
            client
                .call(DaemonIpcRequest::new(
                    "project_sync_status",
                    json!({"project_id": identifier(project_id)?}),
                ))?
                .into_payload::<DaemonSyncStatus>()?
        } else {
            client.sync_status()?
        };
        let retrieval = project
            .map(|project_id| client.search_index_status(SearchIndexProjectRequest { project_id }))
            .transpose()?;
        print_json(
            &json!({"health": client.health()?, "session": client.project_config()?, "sync": sync, "retrieval": retrieval}),
        )?;
        return Ok(());
    }
    if matches!(
        cli.command,
        Command::Daemon {
            action: DaemonCommand::Stop | DaemonCommand::Restart
        }
    ) {
        resident::stop()?;
        if matches!(
            cli.command,
            Command::Daemon {
                action: DaemonCommand::Stop
            }
        ) {
            return print_json(&json!({"stopped": true}));
        }
    }
    let client = resident::ensure_running()?;
    match cli.command {
        Command::Status { .. } => unreachable!(),
        Command::Daemon { .. } => print_json(&client.health()?),
        Command::Login {
            server,
            username,
            password_stdin,
            no_browser,
        } => {
            let server = validated_origin(&server)?;
            let methods = sign_in::login_methods(&server)?;
            let session = if let Some(username) = username {
                if !methods.password_enabled {
                    return Err("This Server does not enable password login".into());
                }
                let password = if password_stdin {
                    read_stdin()?.trim_end_matches(['\r', '\n']).to_owned()
                } else {
                    rpassword::prompt_password("Password: ")?
                };
                sign_in::password_login(&server, &username, &password)?
            } else {
                if !methods.oidc_enabled {
                    return Err("This Server requires --username for password login".into());
                }
                sign_in::authenticate_with_browser(&server, !no_browser)?
            };
            install_session(&client, server, session)
        }
        Command::Redeem {
            server,
            username,
            reset_password,
            stdin,
        } => {
            let server = validated_origin(&server)?;
            let (token, password) = if stdin {
                let input = read_json(PathBuf::from("-"))?;
                (
                    input["token"]
                        .as_str()
                        .ok_or("Input requires token")?
                        .to_owned(),
                    input["password"]
                        .as_str()
                        .ok_or("Input requires password")?
                        .to_owned(),
                )
            } else {
                (
                    rpassword::prompt_password("Invitation/reset token: ")?,
                    rpassword::prompt_password("New password: ")?,
                )
            };
            let session = sign_in::redeem_credential(
                &server,
                &token,
                username.as_deref(),
                &password,
                !reset_password,
            )?;
            install_session(&client, server, session)
        }
        Command::Logout => {
            let previous = client.project_config()?;
            if let Err(error) = server(&client, "DELETE", "/api/v1/auth/session", None, None) {
                eprintln!("Session revocation failed; forgetting local credentials: {error}");
            }
            client.replace_project_config(DaemonProjectConfigUpdateRequest {
                server_url: previous.server_url,
                project_id: None,
                memory_guidelines_path: None,
                access_token: None,
                refresh_token: None,
            })?;
            print_json(&json!({"signed_out": true}))
        }
        Command::Project { action } => match action {
            ProjectCommand::List => {
                print_json(&server(&client, "GET", "/api/v1/projects", None, None)?)
            }
            ProjectCommand::Show { id } => print_json(&server(
                &client,
                "GET",
                &format!("/api/v1/projects/{}", identifier(&id)?),
                None,
                None,
            )?),
            ProjectCommand::Create { name } => print_json(&server(
                &client,
                "POST",
                "/api/v1/projects",
                Some(json!({"name": name})),
                None,
            )?),
            ProjectCommand::Join { id } => {
                server(
                    &client,
                    "GET",
                    &format!("/api/v1/projects/{}", identifier(&id)?),
                    None,
                    None,
                )?;
                print_json(
                    &client.select_project(DaemonProjectSelectionRequest { project_id: id })?,
                )
            }
            ProjectCommand::Bind { id, path, revision } => print_json(
                &client.replace_project_binding(DaemonProjectBindingReplaceRequest {
                    project_id: id,
                    workspace_root: directory(path)?,
                    expected_revision: revision,
                })?,
            ),
            ProjectCommand::Unbind { path, revision } => print_json(
                &client.remove_project_binding(DaemonProjectBindingRemoveRequest {
                    workspace_root: directory(path)?,
                    expected_revision: revision,
                })?,
            ),
            ProjectCommand::Bindings { id } => print_json(
                &client
                    .list_project_bindings(DaemonProjectBindingListRequest { project_id: id })?,
            ),
            ProjectCommand::Current => print_json(
                &DaemonIpcClient::for_agent_runtime(
                    client.service_name(),
                    agent_runtime::current_identity(),
                )
                .resolve_project_binding(DaemonProjectBindingResolveRequest {
                    workspace_path: directory(std::env::current_dir()?)?,
                    required_adapter: None,
                })?,
            ),
        },
        Command::Agent { action } => match action {
            AgentCommand::List => print_json(
                &client
                    .call(DaemonIpcRequest::empty("agent_adapter_settings"))?
                    .into_payload::<Value>()?,
            ),
            AgentCommand::Enable { host, host_binary } => {
                configure_agent(&client, &host, true, host_binary)
            }
            AgentCommand::Disable { host, host_binary } => {
                configure_agent(&client, &host, false, host_binary)
            }
        },
        Command::Draft { action } => match action {
            DraftCommand::Retry { project } => print_json(
                &client
                    .call(DaemonIpcRequest::new(
                        "project_retry_sync",
                        serde_json::to_value(DaemonProjectSyncRetryRequest {
                            project_id: project,
                            channel: SyncRetryChannel::Drafts,
                        })?,
                    ))?
                    .into_payload::<DaemonRetryResponse>()?,
            ),
            DraftCommand::List { cursor } => {
                print_json(&client.list_drafts(DaemonDraftListQuery {
                    cursor,
                    limit: Some(100),
                    ..Default::default()
                })?)
            }
            DraftCommand::Show { id } => print_json(&client.get_draft(id)?),
            DraftCommand::Sync { id } => print_json(&review::uploaded(&client, &id)?),
            DraftCommand::Plan { id } => {
                let draft = review::uploaded(&client, &id)?;
                print_json(&server(
                    &client,
                    "POST",
                    &format!(
                        "/api/v1/drafts/{}/reconciliation-candidates",
                        identifier(
                            draft
                                .server_draft_id
                                .as_deref()
                                .ok_or("Draft has no server identity")?
                        )?
                    ),
                    Some(json!({"expected_draft_version": draft.server_version})),
                    None,
                )?)
            }
            DraftCommand::Rebase {
                id,
                candidate,
                version,
                reference,
                resolved,
            } => {
                let draft = review::uploaded(&client, &id)?;
                print_json(&server(
                    &client,
                    "POST",
                    &format!(
                        "/api/v1/drafts/{}/rebases",
                        identifier(
                            draft
                                .server_draft_id
                                .as_deref()
                                .ok_or("Draft has no server identity")?
                        )?
                    ),
                    Some(
                        json!({"candidate_id": candidate, "expected_draft_version": version, "resolved_state": resolved.map(read_json).transpose()?}),
                    ),
                    Some(&reference),
                )?)
            }
        },
        Command::Review { action } => review::run(&client, action),
    }
}

/// Transfers a login result to the existing credential owner, retaining same-Server client context.
///
/// # Errors
/// Returns daemon transport, metadata, or credential-store failures.
fn install_session(
    client: &DaemonIpcClient,
    server: String,
    session: sign_in::Session,
) -> Result<(), Box<dyn std::error::Error>> {
    let previous = client.project_config()?;
    let same_server = previous.server_url.trim_end_matches('/') == server;
    print_json(
        &client.replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: server,
            project_id: same_server.then_some(previous.project_id).flatten(),
            memory_guidelines_path: same_server
                .then_some(previous.memory_guidelines_path)
                .flatten(),
            access_token: Some(session.access_token),
            refresh_token: session.refresh_token,
        })?,
    )
}

/// Installs or removes an existing daemon-owned global host adapter.
///
/// # Errors
/// Rejects unsupported hosts, modified integration files, or unavailable runtimes.
fn configure_agent(
    client: &DaemonIpcClient,
    host: &str,
    enabled: bool,
    host_binary: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let adapter = match host {
        "codex" => ProjectAgentAdapterKind::Codex,
        "claude-code" => ProjectAgentAdapterKind::ClaudeCode,
        "opencode" => ProjectAgentAdapterKind::Opencode,
        "dsh" => ProjectAgentAdapterKind::Dsh,
        "antigravity" => ProjectAgentAdapterKind::Antigravity,
        _ => return Err("Unsupported Agent host".into()),
    };
    let runtime = resident::binary()?;
    let host_binary = if adapter == ProjectAgentAdapterKind::Codex {
        let candidate = host_binary.or_else(|| std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|directory| directory.join(if cfg!(windows) { "codex.exe" } else { "codex" })).find(|path| path.is_file()))).ok_or("Codex executable was not found; pass --host-binary with its installed executable path")?;
        Some(
            candidate
                .canonicalize()?
                .to_str()
                .ok_or("Codex path must be UTF-8")?
                .to_owned(),
        )
    } else {
        None
    };
    let request = DaemonSetAgentAdapterRequest {
        adapter,
        enabled,
        runtime_binary_path: runtime
            .to_str()
            .ok_or("Runtime path must be UTF-8")?
            .to_owned(),
        host_binary_path: host_binary,
    };
    print_json(
        &client
            .call(DaemonIpcRequest::new(
                "set_agent_adapter",
                serde_json::to_value(request)?,
            ))?
            .into_payload::<Value>()?,
    )
}

/// Accepts only API identifiers, preventing path or query injection.
///
/// # Errors
/// Rejects empty, oversized, or non-identifier input.
fn identifier(value: &str) -> Result<&str, Box<dyn std::error::Error>> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err(
            "Invalid identifier (expected letters, digits, underscores, or hyphens)".into(),
        );
    }
    Ok(value)
}

/// Validates an origin before sending credentials to it.
///
/// # Errors
/// Rejects credentials, paths, fragments, insecure remote HTTP, or malformed URLs.
fn validated_origin(value: &str) -> Result<String, Box<dyn std::error::Error>> {
    let url = reqwest::Url::parse(value.trim())?;
    let loopback = url
        .host_str()
        .is_some_and(|host| host == "localhost" || host == "127.0.0.1" || host == "[::1]");
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err("Use an HTTPS Server origin without credentials or a path; HTTP is allowed only on loopback".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// Resolves a real workspace directory without lossy path conversion.
///
/// # Errors
/// Rejects nonexistent directories or paths the daemon JSON protocol cannot encode.
fn directory(path: PathBuf) -> Result<String, Box<dyn std::error::Error>> {
    let path = path.canonicalize()?;
    if !path.is_dir() {
        return Err("Workspace must be a directory".into());
    }
    Ok(path
        .to_str()
        .ok_or("Workspace path must be UTF-8")?
        .to_owned())
}

/// Reads bounded secret or JSON input from stdin.
///
/// # Errors
/// Rejects oversized or unreadable input.
fn read_stdin() -> Result<String, Box<dyn std::error::Error>> {
    let mut text = String::new();
    io::stdin()
        .take(4 * 1024 * 1024 + 1)
        .read_to_string(&mut text)?;
    if text.len() > 4 * 1024 * 1024 {
        return Err("Input exceeds 4 MiB".into());
    }
    Ok(text)
}

/// Reads an explicit JSON file; '-' selects stdin.
///
/// # Errors
/// Rejects unreadable, oversized, or malformed input.
fn read_json(path: PathBuf) -> Result<Value, Box<dyn std::error::Error>> {
    let text = if path.as_os_str() == "-" {
        read_stdin()?
    } else {
        let file = std::fs::File::open(path)?;
        let mut text = String::new();
        file.take(4 * 1024 * 1024 + 1).read_to_string(&mut text)?;
        if text.len() > 4 * 1024 * 1024 {
            return Err("Input exceeds 4 MiB".into());
        }
        text
    };
    Ok(serde_json::from_str(&text)?)
}

/// Prints stable machine-readable output without emitting credentials.
///
/// # Errors
/// Propagates serialization failure.
fn print_json(value: &impl serde::Serialize) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Executes one authenticated request through the resident's refresh and permission boundary.
///
/// # Errors
/// Preserves HTTP failures and never retries a version-conflicted mutation.
fn server(
    client: &DaemonIpcClient,
    method: &str,
    path: &str,
    body: Option<Value>,
    reference: Option<&str>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut headers = BTreeMap::new();
    if body.is_some() {
        headers.insert("content-type".to_owned(), "application/json".to_owned());
    }
    if let Some(reference) = reference {
        headers.insert(
            "If-Match".to_owned(),
            format!("\"{}\"", identifier(reference)?),
        );
    }
    let response = client.server_request(DaemonServerRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        headers,
        body: body.map(|body| body.to_string()),
    })?;
    if !(200..300).contains(&response.status) {
        let envelope: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
        let code = envelope["error"]["code"].as_str().unwrap_or("server_error");
        let message = envelope["error"]["message"]
            .as_str()
            .unwrap_or("Server request failed");
        let recovery = if response.status == 401 {
            "; run clumsies login again; local drafts are retained"
        } else {
            ""
        };
        return Err(format!("HTTP {} {code}: {message}{recovery}", response.status).into());
    }
    if response.body.trim().is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(&response.body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_and_identifiers_reject_credential_and_path_injection() {
        for origin in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/path",
            "https://example.com?x=1",
        ] {
            assert!(validated_origin(origin).is_err());
        }
        assert_eq!(
            validated_origin("http://127.0.0.1:1234/").unwrap(),
            "http://127.0.0.1:1234"
        );
        for id in ["", "x/y", "x?admin=true", "x\r\nHeader: bad"] {
            assert!(identifier(id).is_err());
        }
    }
}
