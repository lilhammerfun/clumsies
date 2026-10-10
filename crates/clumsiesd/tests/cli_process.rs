//! Isolated CLI/MCP processes using real daemon storage and a checked Server protocol fixture.
#![cfg(not(target_os = "macos"))]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use clumsiesd::{DaemonConfig, DaemonIpcClient};
use serde_json::{Value, json};

/// Disposable protocol state; no authentication or content reaches the user's real runtime.
#[derive(Default)]
struct Protocol {
    /// Uploaded proposal including Server operation shapes.
    draft: Value,
    /// Review lifecycle and concurrency revision.
    review: Value,
    /// Force expired access and refresh credentials for recovery checks.
    expired: bool,
    /// Count refresh attempts without retaining tokens in test logs.
    refreshes: usize,
    /// Allow one real daemon refresh retry before testing revoked credentials.
    recover_refresh: bool,
    /// Inject a non-retryable upload error until the operator explicitly retries.
    fail_upload: bool,
    /// Make Review reads fail after the daemon has cached an authoritative response.
    fail_review_read: bool,
    /// Fail a continuation page after earlier results have been delivered.
    fail_project_page: bool,
}

/// One fixture process and its independent home, daemon database, and IPC endpoint.
struct Fixture {
    /// Owns every on-disk test resource.
    root: tempfile::TempDir,
    /// Resident protocol fixture process.
    child: Child,
    /// Origin of the isolated HTTP service.
    origin: String,
}

impl Fixture {
    /// Starts a re-entered test process so all environment changes remain subprocess-local.
    fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        isolate(&mut command, root.path());
        command
            .args(["--ignored", "--exact", "fixture_process", "--nocapture"])
            .env("CLUMSIES_CLI_FIXTURE", "1");
        let log = std::fs::File::create(root.path().join("fixture.log")).unwrap();
        command.stdout(log.try_clone().unwrap()).stderr(log);
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let ready = root.path().join("ready.json");
        while !ready.exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "fixture exited: {}",
                std::fs::read_to_string(root.path().join("fixture.log")).unwrap()
            );
            assert!(Instant::now() < deadline, "fixture startup timed out");
            std::thread::sleep(Duration::from_millis(50));
        }
        let value: Value = serde_json::from_slice(&std::fs::read(ready).unwrap()).unwrap();
        Self {
            root,
            child,
            origin: value["origin"].as_str().unwrap().to_owned(),
        }
    }

    /// Runs a bounded process with file capture so inherited Windows pipes cannot hide an exit.
    fn output(&self, command: &mut Command, input: Option<&str>) -> Output {
        let name = uuid::Uuid::new_v4().to_string();
        let stdout = self.root.path().join(format!("{name}.stdout"));
        let stderr = self.root.path().join(format!("{name}.stderr"));
        command
            .stdin(Stdio::piped())
            .stdout(std::fs::File::create(&stdout).unwrap())
            .stderr(std::fs::File::create(&stderr).unwrap());
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        if let Some(input) = input {
            stdin.write_all(input.as_bytes()).unwrap();
        }
        drop(stdin);
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                child.kill().unwrap();
                break child.wait().unwrap();
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut stderr = std::fs::read(stderr).unwrap();
        if timed_out {
            stderr.extend_from_slice(
                format!(
                    "\nProcess {:?} exceeded 90 seconds",
                    command.get_args().collect::<Vec<_>>()
                )
                .as_bytes(),
            );
        }
        Output {
            status,
            stdout: std::fs::read(stdout).unwrap(),
            stderr,
        }
    }

    /// Runs the shipped CLI with an isolated stdin and home.
    fn cli(&self, args: &[&str], input: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clumsies"));
        isolate(&mut command, self.root.path());
        self.output(command.args(args), input)
    }

    /// Requires a successful JSON CLI result, retaining useful failure diagnostics.
    fn json(&self, args: &[&str], input: Option<&str>) -> Value {
        let output = self.cli(&[&["--json"], args].concat(), input);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// Starts the existing MCP executable with actual stdio framing and binding enforcement.
    fn mcp(&self, workspace: &Path, arguments: Value) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clumsiesd"));
        isolate(&mut command, self.root.path());
        command.current_dir(workspace).args(["mcp", "serve"]);
        let mut input = String::new();
        for message in [
            json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{"protocolVersion":"2024-11-05", "capabilities":{}, "clientInfo":{"name":"fixture","version":"1"}}}),
            json!({"jsonrpc":"2.0", "method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{"name":"memory", "arguments":arguments}}),
        ] {
            input.push_str(&message.to_string());
            input.push('\n');
        }
        let output = self.output(&mut command, Some(&input));
        assert!(
            output.status.success(),
            "MCP failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|response| response["id"] == 2)
            .unwrap()
    }
}

impl Drop for Fixture {
    /// Reaps the fixture even after a failed assertion.
    fn drop(&mut self) {
        let _ = self.cli(&["logout"], None);
        let _ = self.cli(&["daemon", "stop"], None);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Pins every subprocess path and credential identity to this scenario.
fn isolate(command: &mut Command, root: &Path) {
    for name in [
        "HOME",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "CODEX_HOME",
        "CLUMSIES_DAEMON_ROOT",
        "CLUMSIES_DAEMON_CACHE_DIR",
        "CLUMSIES_DAEMON_LOG_DIR",
        "CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR",
    ] {
        command.env(name, root);
    }
    command
        .env("CLUMSIES_SYNC_ENABLED", "true")
        .env("CLUMSIES_SYNC_INTERVAL_MS", "200")
        .env_remove("CLUMSIES_DEV_INSTANCE_ID")
        .env_remove("CLUMSIES_SERVER_URL")
        .env_remove("CLUMSIES_PROJECT_ID")
        .env_remove("CLUMSIES_MEMORY_GUIDELINES_PATH")
        .env_remove("CLUMSIES_DAEMON_SOCKET")
        .env_remove("CLUMSIES_AGENT_RUNTIME_TEST_MACH_SERVICE");
}

/// Builds a daemon-readable remote proposal shape for actual upload synchronization.
fn remote_draft(request: &Value) -> Value {
    json!({"draft_id":"drf_fixture", "project_id":"prj_fixture", "base_commit_id":null,
        "resource":request["resource"], "status":"open", "version":1,
        "coordination":{"current_commit_id":null,"freshness":"current","has_upstream_resource_changes":false,"reconciliation":"unknown","candidate_id":null},
        "created_at":"2026-10-09T00:00:00Z", "updated_at":"2026-10-09T00:00:00Z"})
}

/// Returns current Review detail in the same proposal order used for submission.
fn review_detail(protocol: &Protocol) -> Value {
    json!({"review":protocol.review, "draft":protocol.draft["draft"], "operations":protocol.draft["operations"], "drafts":[protocol.draft], "comments":[]})
}

/// Responds with the public API error envelope rather than leaking request credentials.
fn failure(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error":{"code":code,"message":code}}))).into_response()
}

/// Protocol substitute checks permissions and revisions; daemon persistence and transports are real.
async fn api(
    State(state): State<Arc<Mutex<Protocol>>>,
    request: axum::extract::Request,
) -> Response {
    let query: BTreeMap<_, _> = reqwest::Url::parse(&format!("http://fixture{}", request.uri()))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let authenticated = request
        .headers()
        .get("authorization")
        .is_some_and(|header| {
            matches!(
                header.to_str(),
                Ok("Bearer fixture-access" | "Bearer fixture-renewed")
            )
        });
    let reference = request
        .headers()
        .get("if-match")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let mut protocol = state.lock().unwrap();
    if path == "/api/v1/auth/methods" {
        return Json(json!({"password_enabled":true,"oidc_enabled":false,"google":false}))
            .into_response();
    }
    if matches!(
        path.as_str(),
        "/api/v1/auth/password/sessions"
            | "/api/v1/auth/invitations/accept"
            | "/api/v1/auth/password/reset"
    ) {
        assert_eq!(body["password"], "fixture-secret");
        if path != "/api/v1/auth/password/sessions" {
            assert_eq!(body["token"], "fixture-invitation");
        }
        protocol.expired = false;
        return Json(json!({"access_token":"fixture-access","refresh_token":"fixture-refresh"}))
            .into_response();
    }
    if path == "/api/v1/auth/token" {
        protocol.refreshes += 1;
        if protocol.recover_refresh {
            protocol.expired = false;
            protocol.recover_refresh = false;
            return Json(
                json!({"access_token":"fixture-renewed","refresh_token":"fixture-renewed-refresh"}),
            )
            .into_response();
        }
        return failure(StatusCode::UNAUTHORIZED, "invalid_refresh_token");
    }
    if path == "/fixture/refresh" {
        protocol.expired = true;
        protocol.recover_refresh = true;
        return Json(json!({})).into_response();
    }
    if path == "/fixture/expire" {
        protocol.expired = true;
        return Json(json!({})).into_response();
    }
    if path == "/fixture/upload-failure" {
        protocol.fail_upload = body["enabled"] == true;
        return Json(json!({})).into_response();
    }
    if path == "/fixture/review-read-failure" {
        protocol.fail_review_read = body["enabled"] == true;
        return Json(json!({})).into_response();
    }
    if path == "/fixture/project-page-failure" {
        protocol.fail_project_page = body["enabled"] == true;
        return Json(json!({})).into_response();
    }
    if protocol.fail_project_page && path == "/api/v1/projects" && query.contains_key("cursor") {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "page_unavailable");
    }
    if protocol.fail_review_read && method == "GET" && path == "/api/v1/reviews/rev_fixture" {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "review_unavailable");
    }
    if !authenticated {
        return failure(StatusCode::UNAUTHORIZED, "missing_session");
    }
    if protocol.expired {
        return failure(StatusCode::UNAUTHORIZED, "missing_session");
    }
    if path.contains("forbidden") {
        return failure(StatusCode::FORBIDDEN, "permission_denied");
    }
    let project = json!({"project_id":"prj_fixture","name":"Fixture","description":"", "revision":1,"created_at":"2026-10-09T00:00:00Z","updated_at":"2026-10-09T00:00:00Z"});
    let result = match (method.as_str(), path.as_str()) {
        ("GET", "/api/v1/me") => json!({"org":{"org_id":"org_fixture"},"projects":[project]}),
        ("GET", "/api/v1/projects") => {
            let other = json!({"project_id":"prj_other","name":"Other"});
            if query.get("limit").map(String::as_str) == Some("1") {
                if let Some(cursor) = query.get("cursor") {
                    assert_eq!(cursor, "next+/&");
                    json!({"items":[project],"page_info":{"has_more":false,"next_cursor":null}})
                } else {
                    json!({"items":[other],"page_info":{"has_more":true,"next_cursor":"next+/&"}})
                }
            } else {
                json!({"items":[other,project],"page_info":{"has_more":false,"next_cursor":null}})
            }
        }
        ("GET", "/api/v1/projects/prj_fixture") => project,
        ("GET", "/api/v1/org/commit-state" | "/api/v1/projects/prj_fixture/commit-state") => {
            let scope = if path == "/api/v1/org/commit-state" {
                "org"
            } else {
                "project"
            };
            let project = if scope == "org" {
                Value::Null
            } else {
                json!("prj_fixture")
            };
            return ([("etag", "\"ref-none\"")], Json(json!({"update_available":false,"ref":{"name":"refs/heads/main","scope":scope,"org_id":"org_fixture","project_id":project,"commit_id":null,"updated_at":"2026-10-09T00:00:00Z"},"latest":null,"download_url":null,"incremental_supported":false}))).into_response();
        }
        ("GET", "/api/v1/draft-events") => json!({"events":[],"next_cursor":null,"has_more":false}),
        ("POST", "/api/v1/drafts") => {
            if protocol.fail_upload {
                return failure(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "content_validation_failed",
                );
            }
            let operations: Vec<_> = body["operations"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(index, operation)| {
                    let mut operation = operation.clone();
                    operation["operation_id"] = json!(format!("op_{index}"));
                    operation["created_at"] = json!("2026-10-09T00:00:00Z");
                    operation
                })
                .collect();
            protocol.draft = json!({"draft":remote_draft(&body),"operations":operations});
            protocol.draft.clone()
        }
        ("GET", "/api/v1/drafts/drf_fixture") => protocol.draft.clone(),
        ("POST", "/api/v1/reviews") => {
            if reference.as_deref() != Some("\"ref-none\"") {
                return failure(StatusCode::PRECONDITION_FAILED, "ref_conflict");
            }
            assert_eq!(body["drafts"][0]["expected_draft_version"], 1);
            protocol.review = json!({"review_id":"rev_fixture","project_id":"prj_fixture","draft_ids":["drf_fixture"],"version":1,"status":"open","title":body["title"],"coordination":protocol.draft["draft"]["coordination"]});
            review_detail(&protocol)
        }
        ("GET", "/api/v1/reviews") => {
            json!({"items":[protocol.review],"page_info":{"has_more":false,"next_cursor":null}})
        }
        ("GET", "/api/v1/reviews/rev_fixture") => review_detail(&protocol),
        ("POST", "/api/v1/reviews/rev_fixture/update-plans") => {
            if body["expected_review_version"] != protocol.review["version"] {
                return failure(StatusCode::CONFLICT, "version_conflict");
            }
            json!({"detail":review_detail(&protocol), "candidates":[{"candidate_id":"candidate_fixture","draft_id":"drf_fixture","draft_version":protocol.draft["draft"]["version"],"current_commit_id":null,"status":"conflicts","valid":true,"merge_preview":{"state":{"exists":true,"resource":protocol.draft["draft"]["resource"],"content":{"content":"# Resolved CLI fixture\n"}}}}]})
        }
        ("POST", "/api/v1/reviews/rev_fixture/updates") => {
            if reference.as_deref() != Some("\"ref-none\"") {
                return failure(StatusCode::PRECONDITION_FAILED, "ref_conflict");
            }
            if body["expected_review_version"] != protocol.review["version"] {
                return failure(StatusCode::CONFLICT, "version_conflict");
            }
            let drafts = body["drafts"].as_array().unwrap();
            if drafts.len() != 1
                || drafts[0]["candidate_id"] != "candidate_fixture"
                || drafts[0]["expected_draft_version"] != protocol.draft["draft"]["version"]
            {
                return failure(StatusCode::CONFLICT, "candidate_stale");
            }
            if drafts[0]["resolved_state"].is_null() {
                return failure(StatusCode::UNPROCESSABLE_ENTITY, "resolution_required");
            }
            protocol.draft["operations"][0]["content"] =
                drafts[0]["resolved_state"]["content"].clone();
            protocol.draft["draft"]["version"] =
                json!(protocol.draft["draft"]["version"].as_i64().unwrap() + 1);
            protocol.review["version"] = json!(protocol.review["version"].as_i64().unwrap() + 1);
            protocol.review["status"] = json!("open");
            review_detail(&protocol)
        }
        ("POST", "/api/v1/reviews/rev_fixture/decisions") => {
            if body["expected_review_version"] != protocol.review["version"] {
                return failure(StatusCode::CONFLICT, "version_conflict");
            }
            protocol.review["status"] = body["decision"].clone();
            protocol.review["version"] = json!(protocol.review["version"].as_i64().unwrap() + 1);
            protocol.review.clone()
        }
        ("POST", "/api/v1/reviews/rev_fixture/merges") => {
            if reference.as_deref() != Some("\"ref-none\"") {
                return failure(StatusCode::PRECONDITION_FAILED, "ref_conflict");
            }
            if body["expected_review_version"] != protocol.review["version"] {
                return failure(StatusCode::CONFLICT, "version_conflict");
            }
            if protocol.review["status"] != "approved" {
                return failure(StatusCode::CONFLICT, "review_not_approved");
            }
            protocol.review["status"] = json!("merged");
            json!({"review":protocol.review,"commit_id":"commit_fixture","applied_operation_count":1})
        }
        ("POST", "/api/v1/reviews/rev_fixture/comments") => {
            if body["expected_review_version"] != protocol.review["version"] {
                return failure(StatusCode::CONFLICT, "version_conflict");
            }
            json!({"body":body["body"]})
        }
        ("DELETE", "/api/v1/auth/session") => json!({}),
        _ => return failure(StatusCode::NOT_FOUND, "fixture_route_not_implemented"),
    };
    Json(result).into_response()
}

/// Child-only fixture entry; explicitly ignored in ordinary test discovery.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "subprocess fixture, invoked by the parent scenario"]
async fn fixture_process() {
    if std::env::var("CLUMSIES_CLI_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let root = PathBuf::from(std::env::var_os("CLUMSIES_DAEMON_ROOT").unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let protocol = Arc::new(Mutex::new(Protocol::default()));
    let app = Router::new().fallback(api).with_state(protocol);
    let _http = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut command = Command::new(env!("CARGO_BIN_EXE_clumsiesd"));
    isolate(&mut command, &root);
    command
        .env("CLUMSIES_SERVER_URL", &origin)
        .env("CLUMSIES_SYNC_ENABLED", "true")
        .env("CLUMSIES_SYNC_INTERVAL_MS", "200");
    if let Some(cache) = std::env::var_os("CLUMSIES_MODEL_TEST_CACHE") {
        command.env("CLUMSIES_DAEMON_CACHE_DIR", cache);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut daemon = command.spawn().unwrap();
    let probe = DaemonIpcClient::new(DaemonConfig::from_env().unwrap().mach_service_name)
        .with_timeout(Duration::from_secs(1));
    let deadline = Instant::now() + Duration::from_secs(30);
    while probe.health().is_err() {
        assert!(
            daemon.try_wait().unwrap().is_none(),
            "resident exited during startup"
        );
        assert!(Instant::now() < deadline, "resident startup timed out");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    std::fs::write(root.join("ready.tmp"), json!({"origin":origin}).to_string()).unwrap();
    std::fs::rename(root.join("ready.tmp"), root.join("ready.json")).unwrap();
    tokio::time::sleep(Duration::from_secs(900)).await;
    let _ = daemon.kill();
    let _ = daemon.wait();
}

/// Exercises human login/binding, MCP authoring/loading, Draft upload, and the Review publication barrier.
#[test]
fn cli_and_mcp_complete_review_and_preserve_work_across_auth_failure() {
    let fixture = Fixture::start();
    let login = fixture.json(
        &[
            "login",
            "--server",
            &fixture.origin,
            "--username",
            "owner",
            "--password-stdin",
        ],
        Some("fixture-secret\n"),
    );
    assert!(login["has_access_token"].as_bool().unwrap());
    assert!(!login.to_string().contains("fixture-access"));
    let first = fixture.json(&["project", "list", "--limit", "1"], None);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let second = fixture.json(
        &[
            "project",
            "list",
            "--limit",
            "1",
            "--cursor",
            first["page_info"]["next_cursor"].as_str().unwrap(),
        ],
        None,
    );
    assert_eq!(second["items"][0]["project_id"], "prj_fixture");
    let all = fixture.json(&["project", "list", "--limit", "1", "--all"], None);
    assert_eq!(all["items"].as_array().unwrap().len(), 2);
    fixture.json(&["project", "select", "fixture"], None);
    let workspace = fixture.root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let binding = fixture.json(
        &[
            "project",
            "bind",
            "prj_fixture",
            workspace.to_str().unwrap(),
        ],
        None,
    );
    let store = fixture.mcp(&workspace, json!({"op":{"store":{"resource":"memory","create":{"path":"guide.md","body":"# CLI fixture\n"}}}}));
    assert_ne!(store["result"]["isError"], true, "{store}");
    let listed = fixture.json(&["draft", "list"], None);
    let draft = listed["items"][0]["draft_id"].as_str().unwrap();
    let open = fixture.json(
        &["draft", "list", "--status", "open", "--limit", "1", "--all"],
        None,
    );
    assert_eq!(open["items"][0]["draft_id"], draft);
    fixture.json(&["draft", "sync", draft], None);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", "--project", "prj_fixture"], None);
        if !status["sync"]["commit_sync"]["last_success_at"].is_null() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Project reference did not synchronize: {status}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let load = fixture.mcp(&workspace, json!({"op":{"load":{"ids":["guide.md"]}}}));
    assert_ne!(load["result"]["isError"], true, "{load}");
    assert!(load.to_string().contains("CLI fixture"));
    let real_models = std::env::var("CLUMSIES_CLI_REAL_MODELS").as_deref() == Ok("1");
    if real_models {
        let deadline = Instant::now() + Duration::from_secs(600);
        loop {
            let status = fixture.json(&["status", "--project", "prj_fixture"], None);
            if status["retrieval"]["ready"] == true {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Model/index preparation timed out: {status}"
            );
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let activate = fixture.mcp(
        &workspace,
        json!({"op":{"activate":{"query":"CLI fixture"}}}),
    );
    assert!(activate["result"].is_object(), "{activate}");
    if real_models {
        assert_ne!(activate["result"]["isError"], true, "{activate}");
        assert!(activate.to_string().contains("guide.md"), "{activate}");
    } else if activate["result"]["isError"] == true {
        assert!(
            activate.to_string().contains("search_model_preparing")
                || activate.to_string().contains("search_index_preparing")
                || activate.to_string().contains("search_index_not_ready"),
            "{activate}"
        );
    } else {
        assert!(activate.to_string().contains("guide.md"), "{activate}");
    }
    let created = fixture.json(
        &["review", "create", draft, "--title", "CLI proposal"],
        None,
    );
    assert_eq!(created["review"]["version"], 1);
    let diff = fixture.cli(&["review", "diff", "rev_fixture"], None);
    assert!(
        diff.status.success(),
        "{}",
        String::from_utf8_lossy(&diff.stderr)
    );
    assert!(String::from_utf8_lossy(&diff.stdout).contains("+# CLI fixture"));
    reqwest::blocking::Client::new()
        .post(format!("{}/fixture/review-read-failure", fixture.origin))
        .json(&json!({"enabled": true}))
        .send()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let stale = fixture.cli(&["review", "diff", "rev_fixture"], None);
        assert!(
            !stale.status.success(),
            "An unavailable Server produced a successful diff"
        );
        if String::from_utf8_lossy(&stale.stderr).contains("cached response is stale") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Cached Review read did not report staleness: {}",
            String::from_utf8_lossy(&stale.stderr)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    reqwest::blocking::Client::new()
        .post(format!("{}/fixture/review-read-failure", fixture.origin))
        .json(&json!({"enabled": false}))
        .send()
        .unwrap();
    let plan = fixture.json(&["review", "plan", "rev_fixture", "--version", "1"], None);
    let request_path = fixture.root.path().join("update.json");
    std::fs::write(&request_path, plan["request"].to_string()).unwrap();
    assert!(
        !fixture
            .cli(
                &[
                    "review",
                    "update",
                    "rev_fixture",
                    "--file",
                    request_path.to_str().unwrap(),
                    "--reference",
                    "ref-none"
                ],
                None
            )
            .status
            .success()
    );
    let mut request = plan["request"].clone();
    request["drafts"][0]["resolved_state"] =
        plan["plan"]["candidates"][0]["merge_preview"]["state"].clone();
    std::fs::write(&request_path, request.to_string()).unwrap();
    let updated = fixture.json(
        &[
            "review",
            "update",
            "rev_fixture",
            "--file",
            request_path.to_str().unwrap(),
            "--reference",
            "ref-none",
        ],
        None,
    );
    assert_eq!(updated["review"]["version"], 2);
    assert!(
        !fixture
            .cli(
                &["review", "approve", "rev_fixture", "--version", "99"],
                None
            )
            .status
            .success()
    );
    assert!(
        !fixture
            .cli(
                &["review", "approve", "rev_forbidden", "--version", "1"],
                None
            )
            .status
            .success()
    );
    fixture.json(
        &[
            "review",
            "comment",
            "rev_fixture",
            "--version",
            "2",
            "Reviewed",
        ],
        None,
    );
    fixture.json(
        &["review", "approve", "rev_fixture", "--version", "2"],
        None,
    );
    assert!(
        !fixture
            .cli(
                &[
                    "review",
                    "merge",
                    "rev_fixture",
                    "--version",
                    "3",
                    "--reference",
                    "commit_stale"
                ],
                None
            )
            .status
            .success()
    );
    let merged = fixture.json(
        &[
            "review",
            "merge",
            "rev_fixture",
            "--version",
            "3",
            "--reference",
            "ref-none",
        ],
        None,
    );
    assert_eq!(merged["review"]["status"], "merged");
    reqwest::blocking::Client::new()
        .post(format!("{}/fixture/refresh", fixture.origin))
        .send()
        .unwrap();
    fixture.json(&["project", "list"], None);
    reqwest::blocking::Client::new()
        .post(format!("{}/fixture/expire", fixture.origin))
        .send()
        .unwrap();
    let expired = fixture.cli(&["project", "list"], None);
    assert!(!expired.status.success());
    assert!(
        String::from_utf8_lossy(&expired.stderr).contains("login"),
        "{}",
        String::from_utf8_lossy(&expired.stderr)
    );
    assert!(
        !fixture.json(&["draft", "list"], None)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fixture.json(
        &[
            "login",
            "--server",
            &fixture.origin,
            "--username",
            "owner",
            "--password-stdin",
        ],
        Some("fixture-secret\n"),
    );
    fixture.json(&["daemon", "restart"], None);
    assert_eq!(
        fixture.json(&["draft", "show", draft], None)["draft"]["draft_id"],
        draft
    );
    assert_eq!(
        fixture.json(&["project", "bindings", "prj_fixture"], None)["items"][0]["revision"],
        binding["revision"]
    );
    fixture.json(
        &[
            "project",
            "unbind",
            workspace.to_str().unwrap(),
            "--revision",
            &binding["revision"].to_string(),
        ],
        None,
    );
    fixture.json(&["logout"], None);
}

/// Human lists traverse pages automatically while explicit JSON retains one-page envelopes.
#[test]
fn text_output_traverses_without_a_pager_and_json_remains_explicit() {
    let fixture = Fixture::start();
    fixture.json(
        &[
            "login",
            "--server",
            &fixture.origin,
            "--username",
            "fixture",
            "--password-stdin",
        ],
        Some("fixture-secret\n"),
    );
    let text = fixture.cli(&["project", "list", "--limit", "1"], None);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("Fixture") && text.contains("Other"));
    assert!(!text.contains("page_info") && !text.contains("next_cursor") && !text.contains("{\""));
    let page = fixture.json(&["project", "list", "--limit", "1"], None);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["page_info"]["has_more"], true);
    let status = fixture.cli(&["status"], None);
    assert!(status.status.success());
    assert!(!String::from_utf8_lossy(&status.stdout).contains("has_access_token"));
    reqwest::blocking::Client::new()
        .post(format!("{}/fixture/project-page-failure", fixture.origin))
        .json(&json!({"enabled":true}))
        .send()
        .unwrap();
    let partial = fixture.cli(&["project", "list", "--limit", "1"], None);
    assert!(!partial.status.success());
    assert!(String::from_utf8_lossy(&partial.stdout).contains("Other"));
    assert!(String::from_utf8_lossy(&partial.stderr).contains("List incomplete after 1 results"));
    let atomic = fixture.cli(
        &["project", "list", "--limit", "1", "--json", "--all"],
        None,
    );
    assert!(!atomic.status.success());
    assert!(
        atomic.stdout.is_empty(),
        "A partial JSON collection was emitted"
    );
}

/// A cold resident must not keep a parent shell's output pipe alive after the CLI exits.
#[test]
fn cold_start_closes_client_output_pipe_while_resident_stays_running() {
    let fixture = Fixture::start();
    let mut command = Command::new(env!("CARGO_BIN_EXE_clumsies"));
    isolate(&mut command, fixture.root.path());
    let mut child = command
        .args(["--json", "daemon", "start"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(fixture.root.path().join("cold-start.stderr")).unwrap())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Cold startup timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "{}",
        std::fs::read_to_string(fixture.root.path().join("cold-start.stderr")).unwrap()
    );
    let bytes = receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("Resident inherited the CLI output pipe")
        .unwrap();
    reader.join().unwrap();
    let health: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        fixture.json(&["daemon", "start"], None)["daemon_installation_id"],
        health["daemon_installation_id"]
    );
}

/// Exercises account admission, adapter ownership, and rejection through shipped processes.
#[test]
fn cli_redeems_installs_host_and_rejects_review() {
    let fixture = Fixture::start();
    fixture.json(
        &[
            "redeem",
            "--server",
            &fixture.origin,
            "--username",
            "owner",
            "--stdin",
        ],
        Some(r#"{"token":"fixture-invitation","password":"fixture-secret"}"#),
    );
    fixture.json(&["project", "join", "prj_fixture"], None);
    let workspace = fixture.root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    fixture.json(
        &[
            "project",
            "bind",
            "prj_fixture",
            workspace.to_str().unwrap(),
        ],
        None,
    );
    let host_config = fixture.root.path().join(".config/opencode/opencode.json");
    std::fs::create_dir_all(host_config.parent().unwrap()).unwrap();
    std::fs::write(&host_config, r#"{"theme":"retained"}"#).unwrap();
    fixture.json(&["agent", "enable", "opencode"], None);
    let installed = std::fs::read_to_string(&host_config).unwrap();
    assert!(installed.contains("clumsiesd") && installed.contains("retained"));
    fixture.json(&["agent", "disable", "opencode"], None);
    let removed = std::fs::read_to_string(&host_config).unwrap();
    assert!(!removed.contains("clumsiesd") && removed.contains("retained"));
    let service = reqwest::blocking::Client::new();
    service
        .post(format!("{}/fixture/upload-failure", fixture.origin))
        .json(&json!({"enabled":true}))
        .send()
        .unwrap();
    let store = fixture.mcp(&workspace, json!({"op":{"store":{"resource":"memory","create":{"path":"reject.md","body":"# Proposed\n"}}}}));
    assert_ne!(store["result"]["isError"], true, "{store}");
    let listed = fixture.json(&["draft", "list"], None);
    let draft = listed["items"][0]["draft_id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while fixture.json(&["draft", "show", draft], None)["draft"]["failed_operation_count"] == 0 {
        assert!(
            Instant::now() < deadline,
            "Injected upload failure did not reach the durable queue"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !fixture
            .cli(
                &["review", "create", draft, "--title", "Failed upload"],
                None
            )
            .status
            .success()
    );
    service
        .post(format!("{}/fixture/upload-failure", fixture.origin))
        .json(&json!({"enabled":false}))
        .send()
        .unwrap();
    fixture.json(&["draft", "retry", "prj_fixture"], None);
    fixture.json(
        &["review", "create", draft, "--title", "Reject proposal"],
        None,
    );
    let rejected = fixture.json(
        &[
            "review",
            "reject",
            "rev_fixture",
            "--version",
            "1",
            "--note",
            "Needs revision",
        ],
        None,
    );
    assert_eq!(rejected["status"], "rejected");
    fixture.json(&["logout"], None);
    fixture.json(
        &[
            "redeem",
            "--server",
            &fixture.origin,
            "--reset-password",
            "--stdin",
        ],
        Some(r#"{"token":"fixture-invitation","password":"fixture-secret"}"#),
    );
}
