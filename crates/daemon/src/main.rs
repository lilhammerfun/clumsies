use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::agent_runtime::hook::{HookHost, MAX_HOOK_INPUT_BYTES, normalize_hook_event};
use daemon::agent_runtime::mcp::McpServer;
use daemon::agent_runtime::{AgentRuntimeBackend, mcp_contract::AgentRuntimeRequest};
use daemon::{
    CredentialStore, CredentialStoreError, DAEMON_MACH_SERVICE_NAME, DEV_INSTANCE_ID_ENV,
    DaemonConfig, DaemonError, DaemonIpcClient, DaemonIpcResponse, DaemonIpcServer,
    DaemonIpcService, DaemonProjectBindingResolveRequest, DaemonState, LaunchAgentConfig,
    LaunchAgentController, ProjectAgentAdapterDelivery, ProjectAgentAdapterKind,
    ProjectAgentAdapterRuntimeRequirement, ServerCredentials,
};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::SystemTime;
use tracing_subscriber::fmt::writer::MakeWriterExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessMode {
    McpServe(Option<ProjectAgentAdapterRuntimeRequirement>),
    AgentRunEvent {
        host: HookHost,
        required_adapter: Option<ProjectAgentAdapterRuntimeRequirement>,
    },
    Daemon,
}

const AGENT_RUNTIME_TEST_MACH_SERVICE_ENV: &str = "CLUMSIES_AGENT_RUNTIME_TEST_MACH_SERVICE";
const AGENT_RUNTIME_TEST_MACH_SERVICE_PREFIX: &str = "ai.clumsies.test.";
const AGENT_RUNTIME_TEST_STALE_TOOL_BUILD_ENV: &str =
    "CLUMSIES_AGENT_RUNTIME_TEST_STALE_TOOL_BUILD_ID";
// A launchd cold start may include the bounded startup credential probe before
// the resident listener is ready. Keep MCP bootstrap finite but long enough to
// survive that supported path; Hook delivery remains deliberately short and
// fail-open below.
const AGENT_RUNTIME_STARTUP_IPC_TIMEOUT: Duration = Duration::from_secs(30);
const AGENT_RUNTIME_MCP_IPC_TIMEOUT: Duration = Duration::from_secs(65);
const AGENT_RUNTIME_HOOK_IPC_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
struct DaemonRuntimeMode {
    mach_service_name: String,
    isolated_test: bool,
}

struct DeliveryCheckedBackend {
    client: DaemonIpcClient,
    workspace_path: String,
    project_id: String,
    required_adapter: Option<ProjectAgentAdapterRuntimeRequirement>,
}

impl AgentRuntimeBackend for DeliveryCheckedBackend {
    fn execute(&self, request: AgentRuntimeRequest) -> Result<DaemonIpcResponse, DaemonError> {
        if let Some(required_adapter) = self.required_adapter {
            let binding =
                self.client
                    .resolve_project_binding(DaemonProjectBindingResolveRequest {
                        workspace_path: self.workspace_path.clone(),
                        required_adapter: Some(required_adapter),
                    })?;
            if binding.project_id != self.project_id {
                return Err(DaemonError::State {
                    code: "project_binding_changed",
                    message: "The Project binding changed after this Agent runtime started; start a new Agent task."
                        .to_owned(),
                });
            }
        }
        self.client.execute(request)
    }

    fn guidelines_path(&self) -> Result<Option<String>, DaemonError> {
        self.client.guidelines_path()
    }

    fn active_project_id(&self) -> Result<Option<String>, DaemonError> {
        self.client.active_project_id()
    }
}

#[derive(Debug)]
struct IsolatedTestCredentialStore;

impl CredentialStore for IsolatedTestCredentialStore {
    fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
        Ok(None)
    }

    fn replace(&self, _credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
        Err(CredentialStoreError::new(
            "credential writes are disabled for the isolated Agent runtime test service",
        ))
    }

    fn clear(&self) -> Result<(), CredentialStoreError> {
        Err(CredentialStoreError::new(
            "credential writes are disabled for the isolated Agent runtime test service",
        ))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match process_mode(&args)? {
        ProcessMode::McpServe(required_adapter) => run_mcp_proxy(required_adapter),
        ProcessMode::AgentRunEvent {
            host,
            required_adapter,
        } => {
            // Lifecycle observation is fail-open. The managed wrapper records
            // bounded diagnostics, while raw Hook input is never echoed here.
            init_hook_tracing();
            if run_hook_proxy(host, required_adapter).is_err() {
                tracing::error!("clumsiesd Hook proxy could not record this lifecycle event");
            }
            Ok(())
        }
        ProcessMode::Daemon => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            match runtime.block_on(run_daemon(args)) {
                Ok(()) => Ok(()),
                Err(error) => {
                    // Startup and fatal runtime errors carry a structured,
                    // traceable record in daemon.log in addition to stderr.
                    tracing::error!(
                        error = %error,
                        "clumsiesd daemon exited with a fatal error"
                    );
                    Err(error)
                }
            }
        }
    }
}

/// Hook proxies run without the daemon's log directory; route bounded
/// diagnostics to stderr with an explicit minimum level so the failure
/// record survives regardless of the invoking host's RUST_LOG.
fn init_hook_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(SystemTime)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

fn process_mode(args: &[String]) -> Result<ProcessMode, Box<dyn std::error::Error>> {
    match args {
        [first, second] if first == "mcp" && second == "serve" => Ok(ProcessMode::McpServe(None)),
        [first, second, host_flag, host, delivery_flag, delivery]
            if first == "mcp"
                && second == "serve"
                && host_flag == "--host"
                && delivery_flag == "--delivery" =>
        {
            let (_, required_adapter) = agent_runtime_requirement(host, delivery)?;
            Ok(ProcessMode::McpServe(Some(required_adapter)))
        }
        [agent, command, flag, host]
            if agent == "_agent" && command == "agent-run-event" && flag == "--host" =>
        {
            let (host, _) = agent_runtime_requirement(host, "legacy-files")?;
            Ok(ProcessMode::AgentRunEvent {
                host,
                required_adapter: None,
            })
        }
        [agent, command, host_flag, host, delivery_flag, delivery]
            if agent == "_agent"
                && command == "agent-run-event"
                && host_flag == "--host"
                && delivery_flag == "--delivery" =>
        {
            let (host, required_adapter) = agent_runtime_requirement(host, delivery)?;
            Ok(ProcessMode::AgentRunEvent {
                host,
                required_adapter: Some(required_adapter),
            })
        }
        _ => Ok(ProcessMode::Daemon),
    }
}

fn agent_runtime_requirement(
    host: &str,
    delivery: &str,
) -> Result<(HookHost, ProjectAgentAdapterRuntimeRequirement), Box<dyn std::error::Error>> {
    let (host, adapter) = match host {
        "codex" => (HookHost::Codex, ProjectAgentAdapterKind::Codex),
        "claude-code" => (HookHost::ClaudeCode, ProjectAgentAdapterKind::ClaudeCode),
        "opencode" => (HookHost::Opencode, ProjectAgentAdapterKind::Opencode),
        "dsh" => (HookHost::Dsh, ProjectAgentAdapterKind::Dsh),
        "antigravity" => (HookHost::Antigravity, ProjectAgentAdapterKind::Antigravity),
        _ => return Err("unsupported Agent runtime host".into()),
    };
    let delivery = match delivery {
        "legacy-files" => ProjectAgentAdapterDelivery::LegacyFiles,
        "host-plugin" => ProjectAgentAdapterDelivery::HostPlugin,
        _ => return Err("unsupported Agent runtime delivery".into()),
    };
    Ok((
        host,
        ProjectAgentAdapterRuntimeRequirement { adapter, delivery },
    ))
}

fn run_mcp_proxy(
    required_adapter: Option<ProjectAgentAdapterRuntimeRequirement>,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_mode = daemon_runtime_mode_from_env()?;
    let client = agent_runtime_client(AGENT_RUNTIME_STARTUP_IPC_TIMEOUT, &runtime_mode);
    verify_agent_runtime(&client)?;
    let workspace_path = std::env::current_dir()?.to_string_lossy().into_owned();
    let binding = client.resolve_project_binding(DaemonProjectBindingResolveRequest {
        workspace_path: workspace_path.clone(),
        required_adapter,
    });
    let project_id = match (binding, required_adapter) {
        (Ok(binding), _) => binding.project_id,
        (Err(error), Some(_)) => return Err(error.into()),
        (Err(_), None) => client
            .project_config()
            .ok()
            .and_then(|cfg| cfg.project_id)
            .unwrap_or_default(),
    };
    // The debug-only test seam changes the backend identity only after the
    // startup health and binding requests. This models a resident replacement
    // between MCP initialize and the next tools/call over real XPC.
    let backend = match stale_tool_identity_for_test(&runtime_mode)? {
        Some(identity) => DaemonIpcClient::for_agent_runtime(client.service_name(), identity)
            .with_timeout(AGENT_RUNTIME_MCP_IPC_TIMEOUT),
        None => client.with_timeout(AGENT_RUNTIME_MCP_IPC_TIMEOUT),
    };
    let backend = DeliveryCheckedBackend {
        client: backend,
        workspace_path,
        project_id: project_id.clone(),
        required_adapter,
    };
    let mut server = McpServer::new(backend, project_id, env!("CARGO_PKG_VERSION"));
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    server.serve(&mut stdin.lock(), &mut stdout.lock())?;
    Ok(())
}

fn run_hook_proxy(
    host: HookHost,
    required_adapter: Option<ProjectAgentAdapterRuntimeRequirement>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut raw = Vec::new();
    std::io::stdin()
        .take((MAX_HOOK_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut raw)?;
    let event = normalize_hook_event(host, &raw)?;

    let workspace_path = match event.workspace_path() {
        Some(path) => path.to_owned(),
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    let runtime_mode = daemon_runtime_mode_from_env()?;
    let client = agent_runtime_client(AGENT_RUNTIME_HOOK_IPC_TIMEOUT, &runtime_mode);
    verify_agent_runtime(&client)?;
    let binding = client.resolve_project_binding(DaemonProjectBindingResolveRequest {
        workspace_path,
        required_adapter,
    })?;
    client.record_agent_run_event(event.to_record_request(&binding.project_id))?;
    Ok(())
}

fn verify_agent_runtime(client: &DaemonIpcClient) -> Result<(), Box<dyn std::error::Error>> {
    let resident = client.health()?.agent_runtime;
    if !agent_runtime_matches(&resident) {
        return Err(
            "Agent runtime does not match the resident daemon; restart Clumsies and the Agent host"
                .into(),
        );
    }
    Ok(())
}

fn agent_runtime_client(
    request_timeout: Duration,
    runtime_mode: &DaemonRuntimeMode,
) -> DaemonIpcClient {
    DaemonIpcClient::for_agent_runtime(
        runtime_mode.mach_service_name.clone(),
        daemon::agent_runtime::current_identity(),
    )
    .with_timeout(request_timeout)
}

/// An isolated Mach service is required by the process/XPC integration test.
/// The strict prefix and character allowlist prevent this seam from targeting
/// the installed service or another user's launchd job.
fn daemon_runtime_mode_from_env() -> Result<DaemonRuntimeMode, DaemonError> {
    let dev_instance_id = DaemonConfig::dev_instance_id_from_env()?;
    daemon_runtime_mode(dev_instance_id.as_deref())
}

fn daemon_runtime_mode(dev_instance_id: Option<&str>) -> Result<DaemonRuntimeMode, DaemonError> {
    let test_service_name = optional_utf8_env(
        AGENT_RUNTIME_TEST_MACH_SERVICE_ENV,
        std::env::var(AGENT_RUNTIME_TEST_MACH_SERVICE_ENV),
    )?;
    resolve_daemon_runtime_mode(test_service_name.as_deref(), dev_instance_id)
}

fn resolve_daemon_runtime_mode(
    test_service_name: Option<&str>,
    dev_instance_id: Option<&str>,
) -> Result<DaemonRuntimeMode, DaemonError> {
    if test_service_name.is_some() && dev_instance_id.is_some() {
        return Err(DaemonError::InvalidConfig(format!(
            "{AGENT_RUNTIME_TEST_MACH_SERVICE_ENV} cannot be combined with {DEV_INSTANCE_ID_ENV}"
        )));
    }
    if let Some(service_name) = test_service_name {
        if !cfg!(debug_assertions) {
            return Err(DaemonError::InvalidConfig(format!(
                "{AGENT_RUNTIME_TEST_MACH_SERVICE_ENV} is unavailable in release builds"
            )));
        }
        validate_test_mach_service_name(service_name)?;
        return Ok(DaemonRuntimeMode {
            mach_service_name: service_name.to_owned(),
            isolated_test: true,
        });
    }
    let mach_service_name = match dev_instance_id {
        Some(instance_id) => DaemonConfig::dev_mach_service_name(instance_id),
        None => Ok(DAEMON_MACH_SERVICE_NAME.to_owned()),
    }?;
    Ok(DaemonRuntimeMode {
        mach_service_name,
        isolated_test: false,
    })
}

fn optional_utf8_env(
    name: &str,
    value: Result<String, std::env::VarError>,
) -> Result<Option<String>, DaemonError> {
    match value {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(DaemonError::InvalidConfig(format!(
            "{name} must be valid UTF-8"
        ))),
    }
}

fn validate_test_mach_service_name(service_name: &str) -> Result<(), DaemonError> {
    let valid = service_name.starts_with(AGENT_RUNTIME_TEST_MACH_SERVICE_PREFIX)
        && service_name.len() <= 255
        && service_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if !valid {
        return Err(DaemonError::InvalidConfig(format!(
            "{AGENT_RUNTIME_TEST_MACH_SERVICE_ENV} must be a bounded {AGENT_RUNTIME_TEST_MACH_SERVICE_PREFIX}* service name"
        )));
    }
    Ok(())
}

fn stale_tool_identity_for_test(
    runtime_mode: &DaemonRuntimeMode,
) -> Result<Option<daemon::AgentRuntimeIdentity>, DaemonError> {
    let Some(build_id) = optional_utf8_env(
        AGENT_RUNTIME_TEST_STALE_TOOL_BUILD_ENV,
        std::env::var(AGENT_RUNTIME_TEST_STALE_TOOL_BUILD_ENV),
    )?
    else {
        return Ok(None);
    };
    if !cfg!(debug_assertions)
        || !runtime_mode.isolated_test
        || build_id.is_empty()
        || build_id.len() > 128
        || !build_id.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(DaemonError::InvalidConfig(format!(
            "{AGENT_RUNTIME_TEST_STALE_TOOL_BUILD_ENV} is allowed only for a bounded debug test service build id"
        )));
    }
    Ok(Some(daemon::AgentRuntimeIdentity {
        protocol_revision: daemon::agent_runtime::AGENT_RUNTIME_PROTOCOL_REVISION,
        build_id,
    }))
}

fn agent_runtime_matches(resident: &daemon::AgentRuntimeIdentity) -> bool {
    resident.protocol_revision == daemon::agent_runtime::AGENT_RUNTIME_PROTOCOL_REVISION
        && resident.build_id == daemon::agent_runtime::AGENT_RUNTIME_BUILD_ID
}

async fn run_daemon(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config = DaemonConfig::from_env()?;
    let runtime_mode = daemon_runtime_mode(config.dev_instance_id.as_deref())?;
    let mach_service_name = runtime_mode.mach_service_name.clone();

    let log_file = Mutex::new(open_daemon_log(&config.log_dir)?);
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(SystemTime)
        .with_writer(std::io::stderr.and(log_file))
        .with_target(false)
        .init();
    if !args.is_empty() {
        let mut launch_agent =
            LaunchAgentConfig::from_daemon_config(&config, std::env::current_exe()?)?;
        launch_agent.mach_service_name = mach_service_name.clone();
        let launch_agent_controller =
            LaunchAgentController::for_current_user(launch_agent.clone())?;

        match args.as_slice() {
            [command] if command == "--print-launch-agent-plist" => {
                print!("{}", launch_agent.plist_contents());
                return Ok(());
            }
            [command] if command == "--install-launch-agent" => {
                print_status(&launch_agent_controller.install()?)?;
                return Ok(());
            }
            [command] if command == "--status-launch-agent" => {
                print_status(&launch_agent_controller.status()?)?;
                return Ok(());
            }
            [command] if command == "--bootstrap-launch-agent" => {
                print_status(&launch_agent_controller.bootstrap()?)?;
                return Ok(());
            }
            [command] if command == "--bootout-launch-agent" => {
                print_status(&launch_agent_controller.bootout()?)?;
                return Ok(());
            }
            [command] if command == "--restart-launch-agent" => {
                print_status(&launch_agent_controller.kickstart()?)?;
                return Ok(());
            }
            [command] if command == "--reconcile-launch-agent" => {
                print_status(&launch_agent_controller.reconcile()?)?;
                return Ok(());
            }
            [] => unreachable!(),
            _ => {
                tracing::error!(
                    "usage: clumsiesd [mcp serve|_agent agent-run-event --host <host>|--print-launch-agent-plist|--install-launch-agent|--status-launch-agent|--bootstrap-launch-agent|--bootout-launch-agent|--restart-launch-agent|--reconcile-launch-agent]"
                );
                std::process::exit(64);
            }
        }
    }

    /// Structured crash observability for the resident daemon.
    ///
    /// Hard signal faults (e.g. stack overflow) bypass Rust panics entirely and
    /// leave no record in the daemon logs; macOS still writes .ips crash reports
    /// to ~/Library/Logs/DiagnosticReports. This installs a panic hook that
    /// records structured panic details, and detects rapid restarts (crash
    /// loops) at startup so a dying daemon is visible in the logs.
    fn install_crash_observability(config: &DaemonConfig) {
        use std::io::Write;

        let crash_log_path = config.log_dir.join("clumsiesd.crash.log");

        // A restart shortly after the previous start almost certainly means the
        // previous instance died unexpectedly.
        let marker_path = config.root_dir.join(".daemon-started-at");
        let previous = std::fs::read_to_string(&marker_path)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        if let Some(previous) = previous {
            let gap = now.saturating_sub(previous);
            if gap < 120 {
                tracing::warn!(
                    gap_secs = gap,
                    build_id = daemon::agent_runtime::AGENT_RUNTIME_BUILD_ID,
                    "clumsiesd restarted {gap}s after its previous start; possible crash loop (see ~/Library/Logs/DiagnosticReports/clumsiesd-*.ips)"
                );
            }
        }
        let _ = std::fs::write(&marker_path, now.to_string());

        std::panic::set_hook(Box::new(move |info| {
            let backtrace = std::backtrace::Backtrace::force_capture();
            tracing::error!(
                panic = %info,
                version = env!("CARGO_PKG_VERSION"),
                build_id = daemon::agent_runtime::AGENT_RUNTIME_BUILD_ID,
                "clumsiesd panicked:\n{backtrace}"
            );
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&crash_log_path)
            {
                let _ = writeln!(
                    file,
                    "clumsiesd panicked (pid {} version {} build_id {}): {info}",
                    std::process::id(),
                    env!("CARGO_PKG_VERSION"),
                    daemon::agent_runtime::AGENT_RUNTIME_BUILD_ID
                );
                let _ = writeln!(file, "{backtrace}");
            }
        }));
    }

    install_crash_observability(&config);

    let state = if runtime_mode.isolated_test {
        DaemonState::initialize_with_credential_store(config, Arc::new(IsolatedTestCredentialStore))
            .await?
    } else {
        DaemonState::initialize(config).await?
    };
    let service = DaemonIpcService::new(state.clone());
    let _ipc_server = DaemonIpcServer::start(mach_service_name.clone(), service.clone())?;
    // The unique test service exercises the real process/XPC boundary while
    // deliberately avoiding network sync and model downloads in user space.
    // Dev instances are not test services and retain every production worker.
    let _sync_worker = (!runtime_mode.isolated_test).then(|| state.start_sync_worker());
    let _search_model_worker =
        (!runtime_mode.isolated_test).then(|| state.start_search_model_worker());
    let _search_index_worker =
        (!runtime_mode.isolated_test).then(|| state.start_search_index_worker());
    let _run_reaper = (!runtime_mode.isolated_test).then(|| state.start_run_reaper());
    let health = service.health().await;

    tracing::info!(
        "clumsiesd initialized for Mach service {} with installation {}",
        mach_service_name,
        health.daemon_installation_id
    );

    shutdown_signal().await;
    Ok(())
}

fn open_daemon_log(log_dir: &Path) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(log_dir)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("daemon.log"))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn print_status(status: &daemon::DaemonBootstrapStatus) -> Result<(), serde_json::Error> {
    println!("{}", serde_json::to_string_pretty(status)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_log_open_creates_the_log_directory() {
        let root = tempfile::tempdir().unwrap();
        let log_dir = root.path().join("nested/logs");

        let _log = open_daemon_log(&log_dir).unwrap();

        assert!(log_dir.join("daemon.log").is_file());
    }

    #[test]
    fn proxy_modes_are_parsed_before_daemon_initialization() {
        assert_eq!(
            process_mode(&["mcp".to_owned(), "serve".to_owned()]).unwrap(),
            ProcessMode::McpServe(None)
        );
        assert_eq!(
            process_mode(&[
                "_agent".to_owned(),
                "agent-run-event".to_owned(),
                "--host".to_owned(),
                "codex".to_owned(),
            ])
            .unwrap(),
            ProcessMode::AgentRunEvent {
                host: HookHost::Codex,
                required_adapter: None,
            }
        );
        let plugin_requirement = ProjectAgentAdapterRuntimeRequirement {
            adapter: ProjectAgentAdapterKind::Codex,
            delivery: ProjectAgentAdapterDelivery::HostPlugin,
        };
        assert_eq!(
            process_mode(&[
                "mcp".to_owned(),
                "serve".to_owned(),
                "--host".to_owned(),
                "codex".to_owned(),
                "--delivery".to_owned(),
                "host-plugin".to_owned(),
            ])
            .unwrap(),
            ProcessMode::McpServe(Some(plugin_requirement))
        );
        assert_eq!(
            process_mode(&[
                "_agent".to_owned(),
                "agent-run-event".to_owned(),
                "--host".to_owned(),
                "codex".to_owned(),
                "--delivery".to_owned(),
                "host-plugin".to_owned(),
            ])
            .unwrap(),
            ProcessMode::AgentRunEvent {
                host: HookHost::Codex,
                required_adapter: Some(plugin_requirement),
            }
        );
        assert_eq!(process_mode(&[]).unwrap(), ProcessMode::Daemon);
    }

    #[test]
    fn proxy_rejects_a_different_resident_build() {
        assert!(agent_runtime_matches(&daemon::AgentRuntimeIdentity {
            protocol_revision: daemon::agent_runtime::AGENT_RUNTIME_PROTOCOL_REVISION,
            build_id: daemon::agent_runtime::AGENT_RUNTIME_BUILD_ID.to_owned(),
        }));
        assert!(!agent_runtime_matches(&daemon::AgentRuntimeIdentity {
            protocol_revision: daemon::agent_runtime::AGENT_RUNTIME_PROTOCOL_REVISION,
            build_id: "different-build".to_owned(),
        }));
    }

    #[test]
    fn test_mach_service_seam_cannot_target_the_installed_daemon() {
        let error = validate_test_mach_service_name(DAEMON_MACH_SERVICE_NAME).unwrap_err();
        assert!(matches!(error, DaemonError::InvalidConfig(_)));
        assert!(validate_test_mach_service_name("ai.clumsies.test.runtime_e2e_1").is_ok());
    }

    #[test]
    fn dev_proxy_uses_its_derived_service_without_entering_the_test_seam() {
        let dev = resolve_daemon_runtime_mode(None, Some("a1b2c3d4e5f6")).unwrap();
        assert_eq!(dev.mach_service_name, "ai.clumsies.daemon.dev.a1b2c3d4e5f6");
        assert!(!dev.isolated_test);

        let stable = resolve_daemon_runtime_mode(None, None).unwrap();
        assert_eq!(stable.mach_service_name, DAEMON_MACH_SERVICE_NAME);
        assert!(!stable.isolated_test);
    }

    #[test]
    fn test_mach_service_and_dev_identity_are_mutually_exclusive() {
        let error =
            resolve_daemon_runtime_mode(Some("ai.clumsies.test.issue073"), Some("a1b2c3d4e5f6"))
                .unwrap_err();
        assert!(matches!(error, DaemonError::InvalidConfig(_)));
        assert!(
            error
                .to_string()
                .contains(AGENT_RUNTIME_TEST_MACH_SERVICE_ENV)
        );
        assert!(error.to_string().contains(DEV_INSTANCE_ID_ENV));
    }

    #[test]
    fn validated_test_mode_drives_both_endpoint_and_isolation() {
        let mode = resolve_daemon_runtime_mode(Some("ai.clumsies.test.issue073"), None).unwrap();
        assert_eq!(mode.mach_service_name, "ai.clumsies.test.issue073");
        assert!(mode.isolated_test);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_test_service_cannot_fall_back_to_the_stable_endpoint() {
        use std::os::unix::ffi::OsStringExt;

        let error = optional_utf8_env(
            AGENT_RUNTIME_TEST_MACH_SERVICE_ENV,
            Err(std::env::VarError::NotUnicode(
                std::ffi::OsString::from_vec(vec![0xff]),
            )),
        )
        .unwrap_err();
        assert!(matches!(error, DaemonError::InvalidConfig(_)));
        assert!(
            error
                .to_string()
                .contains(AGENT_RUNTIME_TEST_MACH_SERVICE_ENV)
        );
    }
}
