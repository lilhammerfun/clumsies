#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use daemon::{DaemonHealth, DaemonIpcClient, DaemonProjectBindingResolveRequest};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

const SERVER_URL: &str = "http://127.0.0.1:9";
const PROJECT_ID: &str = "prj_agent_runtime_e2e";
const REBOUND_PROJECT_ID: &str = "prj_agent_runtime_rebound";
const TEST_SERVICE_ENV: &str = "CLUMSIES_AGENT_RUNTIME_TEST_MACH_SERVICE";
const STALE_TOOL_BUILD_ENV: &str = "CLUMSIES_AGENT_RUNTIME_TEST_STALE_TOOL_BUILD_ID";

struct LaunchdJob {
    target: String,
    plist_path: PathBuf,
}

impl LaunchdJob {
    fn bootstrap(binary: &Path, root: &Path, service_name: &str) -> Self {
        let uid = unsafe { libc::geteuid() };
        let domain = format!("gui/{uid}");
        let target = format!("{domain}/{service_name}");
        let cache_dir = root.join("cache");
        let log_dir = root.join("logs");
        let launch_agents_dir = root.join("LaunchAgents");
        for directory in [root, &cache_dir, &log_dir, &launch_agents_dir] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let plist_path = launch_agents_dir.join(format!("{service_name}.plist"));
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key><array><string>{binary}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>MachServices</key><dict><key>{service}</key><true/></dict>
  <key>EnvironmentVariables</key>
  <dict>
    <key>{test_service_env}</key><string>{service}</string>
    <key>CLUMSIES_DAEMON_ROOT</key><string>{root}</string>
    <key>CLUMSIES_DAEMON_CACHE_DIR</key><string>{cache}</string>
    <key>CLUMSIES_DAEMON_LOG_DIR</key><string>{logs}</string>
    <key>CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR</key><string>{launch_agents}</string>
    <key>CLUMSIES_SERVER_URL</key><string>{server_url}</string>
    <key>CLUMSIES_SYNC_ENABLED</key><string>false</string>
    <key>HOME</key><string>{root}</string>
    <key>RUST_LOG</key><string>warn</string>
  </dict>
  <key>StandardOutPath</key><string>{stdout}</string>
  <key>StandardErrorPath</key><string>{stderr}</string>
</dict>
</plist>
"#,
            label = xml_escape(service_name),
            binary = xml_escape(&binary.display().to_string()),
            service = xml_escape(service_name),
            test_service_env = TEST_SERVICE_ENV,
            root = xml_escape(&root.display().to_string()),
            cache = xml_escape(&cache_dir.display().to_string()),
            logs = xml_escape(&log_dir.display().to_string()),
            launch_agents = xml_escape(&launch_agents_dir.display().to_string()),
            server_url = SERVER_URL,
            stdout = xml_escape(&log_dir.join("launchd.stdout.log").display().to_string()),
            stderr = xml_escape(&log_dir.join("launchd.stderr.log").display().to_string()),
        );
        std::fs::write(&plist_path, plist).unwrap();

        let output = Command::new("/bin/launchctl")
            .args(["bootstrap", &domain])
            .arg(&plist_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Self { target, plist_path }
    }
}

impl Drop for LaunchdJob {
    fn drop(&mut self) {
        let _ = Command::new("/bin/launchctl")
            .args(["bootout", &self.target])
            .output();
        let _ = std::fs::remove_file(&self.plist_path);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn real_clumsiesd_process_proxies_use_xpc_and_reject_stale_identity() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let daemon_root = fixture.path().join("daemon");
    std::fs::create_dir_all(&workspace).unwrap();
    let suffix = Uuid::new_v4().simple().to_string();
    let service_name = format!("ai.clumsies.test.runtime.{suffix}");
    let binary = stage_test_binary(
        Path::new(env!("CARGO_BIN_EXE_clumsiesd")),
        &daemon_root.join("bin/clumsiesd"),
    );
    let job = LaunchdJob::bootstrap(&binary, &daemon_root, &service_name);

    let ordinary_client = DaemonIpcClient::new(&service_name).with_timeout(Duration::from_secs(30));
    let health = wait_for_health(&ordinary_client, &daemon_root, &job.target);
    assert_eq!(health.agent_runtime.protocol_revision, 1);
    assert!(!health.agent_runtime.build_id.is_empty());
    let resident_identity = health.agent_runtime;

    seed_project_fixture(&daemon_root.join("local.db"), &workspace).await;

    let mcp = run_proxy(
        &binary,
        &["mcp", "serve"],
        &workspace,
        &service_name,
        concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"memory\",\"arguments\":{\"op\":{\"activate\":{\"query\":\"agent runtime e2e\"}}}}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"memory\",\"arguments\":{\"op\":{\"load\":{\"ids\":[\"workflow/CODING.md\"]}}}}}\n"
        ),
        &[],
    );
    assert_process_succeeded("MCP proxy", &mcp);
    let responses = json_lines(&mcp.stdout);
    assert_eq!(responses.len(), 4);
    assert_eq!(
        response_with_id(&responses, 1)["result"]["protocolVersion"],
        "2025-06-18"
    );
    let tools = response_with_id(&responses, 2)["result"]["tools"]
        .as_array()
        .unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "memory");
    for (id, expected_code) in [(4, "project_ref_not_synced"), (5, "project_ref_not_synced")] {
        let response = response_with_id(&responses, id);
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"], expected_code,
            "{response}"
        );
    }

    let mut plugin_mcp = spawn_proxy(
        &binary,
        &[
            "mcp",
            "serve",
            "--host",
            "codex",
            "--delivery",
            "host-plugin",
        ],
        &workspace,
        &service_name,
        &[],
    );
    let mut plugin_stdin = plugin_mcp.stdin.take().unwrap();
    let mut plugin_stdout = BufReader::new(plugin_mcp.stdout.take().unwrap());
    plugin_stdin
        .write_all(
            concat!(
                "{\"jsonrpc\":\"2.0\",\"id\":20,\"method\":\"initialize\"}\n",
                "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
                "{\"jsonrpc\":\"2.0\",\"id\":21,\"method\":\"tools/call\",\"params\":{\"name\":\"memory\",\"arguments\":{\"op\":{\"activate\":{\"query\":\"binding check\"}}}}}\n"
            )
            .as_bytes(),
        )
        .unwrap();
    plugin_stdin.flush().unwrap();
    assert_eq!(read_proxy_json_line(&mut plugin_stdout)["id"], 20);
    let before_rebind = read_proxy_json_line(&mut plugin_stdout);
    assert_eq!(before_rebind["id"], 21);
    assert_eq!(before_rebind["result"]["isError"], true, "{before_rebind}");
    assert_eq!(
        before_rebind["result"]["structuredContent"]["error"]["code"],
        "project_ref_not_synced"
    );

    let unbound = tempfile::tempdir().unwrap();
    let unbound_output = run_proxy(
        &binary,
        &["mcp", "serve"],
        unbound.path(),
        &service_name,
        "",
        &[],
    );
    assert!(
        !unbound_output.status.success(),
        "An unbound global MCP must not use the App's selected Project"
    );

    let mut direct_mcp = spawn_proxy(&binary, &["mcp", "serve"], &workspace, &service_name, &[]);
    let mut direct_stdin = direct_mcp.stdin.take().unwrap();
    let mut direct_stdout = BufReader::new(direct_mcp.stdout.take().unwrap());
    direct_stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":30,\"method\":\"initialize\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").unwrap();
    direct_stdin.flush().unwrap();
    assert_eq!(read_proxy_json_line(&mut direct_stdout)["id"], 30);

    rebind_project(&daemon_root.join("local.db")).await;

    direct_stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":31,\"method\":\"tools/call\",\"params\":{\"name\":\"memory\",\"arguments\":{\"op\":{\"activate\":{\"query\":\"binding check\"}}}}}\n").unwrap();
    direct_stdin.flush().unwrap();
    let direct_rebind = read_proxy_json_line(&mut direct_stdout);
    assert_eq!(
        direct_rebind["result"]["structuredContent"]["error"]["code"],
        "project_binding_changed"
    );
    drop(direct_stdin);
    drop(direct_stdout);
    assert_process_succeeded("direct global MCP", &direct_mcp.wait_with_output().unwrap());
    plugin_stdin
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":22,\"method\":\"tools/call\",\"params\":{\"name\":\"memory\",\"arguments\":{\"op\":{\"activate\":{\"query\":\"binding check\"}}}}}\n",
        )
        .unwrap();
    plugin_stdin.flush().unwrap();
    let after_rebind = read_proxy_json_line(&mut plugin_stdout);
    assert_eq!(after_rebind["id"], 22);
    assert_eq!(after_rebind["result"]["isError"], true, "{after_rebind}");
    drop(plugin_stdin);
    drop(plugin_stdout);
    let plugin_output = plugin_mcp.wait_with_output().unwrap();
    assert_process_succeeded("delivery-gated MCP proxy", &plugin_output);

    let hook_fixture = json!({
        "session_id": "runtime-e2e-session",
        "turn_id": "runtime-e2e-turn",
        "hook_event_name": "UserPromptSubmit",
        "cwd": workspace.display().to_string(),
        "prompt": "SECRET_HOOK_PROMPT_MUST_NOT_PERSIST",
        "transcript_path": "/private/secret/transcript.jsonl"
    });
    let hook = run_proxy(
        &binary,
        &["_agent", "agent-run-event", "--host", "codex"],
        &workspace,
        &service_name,
        &serde_json::to_string(&hook_fixture).unwrap(),
        &[],
    );
    assert_process_succeeded("Hook proxy", &hook);
    assert!(hook.stdout.is_empty());
    assert_hook_fixture_persisted(&daemon_root.join("local.db")).await;

    let stale_mcp = run_proxy(
        &binary,
        &["mcp", "serve"],
        &workspace,
        &service_name,
        concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":10,\"method\":\"initialize\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":11,\"method\":\"tools/call\",\"params\":{\"name\":\"memory\",\"arguments\":{\"op\":{\"activate\":{\"query\":\"stale runtime\"}}}}}\n"
        ),
        &[(STALE_TOOL_BUILD_ENV, "stale-proxy-build")],
    );
    assert_process_succeeded("stale MCP proxy", &stale_mcp);
    let stale_responses = json_lines(&stale_mcp.stdout);
    assert_eq!(stale_responses.len(), 2);
    assert_eq!(
        response_with_id(&stale_responses, 10)["result"]["protocolVersion"],
        "2025-06-18"
    );
    let rejected = response_with_id(&stale_responses, 11);
    assert_eq!(rejected["result"]["isError"], true, "{rejected}");
    assert_eq!(
        rejected["result"]["structuredContent"]["error"]["code"], "agent_runtime_mismatch",
        "{rejected}"
    );
    assert!(!rejected.to_string().contains("stale-proxy-build"));

    // A rejected Agent request is scoped to that dispatch; ordinary App IPC
    // remains compatible and the resident process remains healthy.
    assert_eq!(
        ordinary_client.health().unwrap().agent_runtime,
        resident_identity
    );
    let missing_identity = ordinary_client
        .resolve_project_binding(DaemonProjectBindingResolveRequest {
            workspace_path: workspace.display().to_string(),
            required_adapter: None,
        })
        .unwrap_err();
    assert!(
        missing_identity
            .to_string()
            .contains("agent_runtime_mismatch"),
        "{missing_identity}"
    );
}

fn stage_test_binary(source: &Path, destination: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::copy(source, destination).unwrap();
    std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o755)).unwrap();
    let signed = Command::new("/usr/bin/codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--identifier",
            "ai.clumsies.daemon",
        ])
        .arg(destination)
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "failed to sign staged test daemon: {}",
        String::from_utf8_lossy(&signed.stderr)
    );
    destination.to_path_buf()
}

fn wait_for_health(
    client: &DaemonIpcClient,
    daemon_root: &Path,
    launchd_target: &str,
) -> DaemonHealth {
    let deadline = Instant::now() + Duration::from_secs(15);
    let probe_client = client.clone().with_timeout(Duration::from_millis(500));
    let mut last_error = None;
    while Instant::now() < deadline {
        match probe_client.health() {
            Ok(health) => return health,
            Err(error) => last_error = Some(error.to_string()),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let log_dir = daemon_root.join("logs");
    let daemon_log = std::fs::read_to_string(log_dir.join("daemon.log")).unwrap_or_default();
    let stdout = std::fs::read_to_string(log_dir.join("launchd.stdout.log")).unwrap_or_default();
    let stderr = std::fs::read_to_string(log_dir.join("launchd.stderr.log")).unwrap_or_default();
    let launchd = Command::new("/bin/launchctl")
        .args(["print", launchd_target])
        .output()
        .map(|output| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .unwrap_or_else(|error| format!("launchctl print failed: {error}"));
    panic!(
        "test resident never became healthy: {}\ndaemon.log:\n{daemon_log}\nlaunchd stdout:\n{stdout}\nlaunchd stderr:\n{stderr}\nlaunchd state:\n{launchd}",
        last_error.unwrap_or_else(|| "no XPC response".to_owned())
    );
}

async fn seed_project_fixture(local_db: &Path, workspace: &Path) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", local_db.display()))
        .await
        .unwrap();
    let workspace = std::fs::canonicalize(workspace).unwrap();
    sqlx::query(
        "INSERT INTO project_bindings (server_url, workspace_root, project_id, revision)
         VALUES ($1, $2, $3, 1)",
    )
    .bind(SERVER_URL)
    .bind(workspace.display().to_string())
    .bind(PROJECT_ID)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
}

async fn rebind_project(local_db: &Path) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", local_db.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE project_bindings SET project_id = $1")
        .bind(REBOUND_PROJECT_ID)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

async fn assert_hook_fixture_persisted(local_db: &Path) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", local_db.display()))
        .await
        .unwrap();
    let run = sqlx::query(
        "SELECT project_id, host, host_run_key, host_session_id, kind, phase, summary
         FROM agent_runs WHERE host_session_id = 'runtime-e2e-session'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(run.get::<String, _>("project_id"), REBOUND_PROJECT_ID);
    assert_eq!(run.get::<String, _>("host"), "codex");
    assert_eq!(
        run.get::<String, _>("host_run_key"),
        "root:runtime-e2e-turn"
    );
    assert_eq!(run.get::<String, _>("kind"), "root");
    assert_eq!(run.get::<String, _>("phase"), "running");
    assert_eq!(run.get::<Option<String>, _>("summary"), None);

    let event = sqlx::query(
        "SELECT source, event_type, summary FROM agent_run_events
         WHERE run_id = (SELECT run_id FROM agent_runs
                         WHERE host_session_id = 'runtime-e2e-session')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event.get::<String, _>("source"), "hook");
    assert_eq!(event.get::<String, _>("event_type"), "started");
    assert_eq!(event.get::<Option<String>, _>("summary"), None);
    pool.close().await;
}

fn run_proxy(
    binary: &Path,
    args: &[&str],
    workspace: &Path,
    service_name: &str,
    input: &str,
    extra_env: &[(&str, &str)],
) -> Output {
    let mut child = spawn_proxy(binary, args, workspace, service_name, extra_env);
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    panic!(
        "proxy did not exit; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn spawn_proxy(
    binary: &Path,
    args: &[&str],
    workspace: &Path,
    service_name: &str,
    extra_env: &[(&str, &str)],
) -> Child {
    Command::new(binary)
        .args(args)
        .current_dir(workspace)
        .env(TEST_SERVICE_ENV, service_name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(extra_env.iter().copied())
        .spawn()
        .unwrap()
}

fn read_proxy_json_line(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    assert!(reader.read_line(&mut line).unwrap() > 0);
    serde_json::from_str(&line).unwrap()
}

fn assert_process_succeeded(name: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{name} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn json_lines(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn response_with_id(responses: &[Value], id: u64) -> &Value {
    responses
        .iter()
        .find(|response| response["id"] == id)
        .unwrap()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
