use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;

use crate::retrieval_history::{RetrievalCandidate, RetrievalRunRequest, RetrievalRunStatus};
use crate::util::home_dir;
use crate::work_tracking::AgentRunHost;
use crate::{DaemonError, DaemonState, SourceScope};

mod codex;

fn run_status_str(status: RetrievalRunStatus) -> &'static str {
    match status {
        RetrievalRunStatus::Running => "running",
        RetrievalRunStatus::Succeeded => "succeeded",
        RetrievalRunStatus::Failed => "failed",
    }
}

/// Upper bound on the number of sessions a single Activity list returns, newest
/// first. The panel is a diagnostic surface, not an unbounded archive dump.
const DEFAULT_SESSION_LIMIT: usize = 50;
const MAX_SESSION_LIMIT: usize = 200;
const MAX_TASKS_PER_SESSION: usize = 500;
const MAX_ACTIVATIONS_PER_TASK: usize = 100;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ListRecallsRequest {
    /// Optional workspace root filter. When omitted, every bound workspace is
    /// included.
    #[serde(default)]
    pub workspace_root: Option<String>,
    /// Optional Project filter. All repositories bound to the Project are
    /// included.
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ListRecallsResponse {
    pub sessions: Vec<RecallSession>,
    pub workspace_roots: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GetRecallFragmentRequest {
    pub workspace_root: String,
    pub run_id: String,
    pub unit_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GetRecallFragmentResponse {
    pub fragment: RecallFragment,
}

/// A dsh session and the tasks it contained, with each task's memory
/// activations attached.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecallSession {
    /// Harness that produced this session log. Different providers normalize
    /// into the same session -> task -> memory-activation projection.
    pub host: AgentRunHost,
    pub session_id: String,
    pub title: Option<String>,
    pub workspace_root: String,
    pub created_at: Option<i64>,
    pub tasks: Vec<RecallTask>,
}

/// One user prompt (task) and the memory activations the agent issued while
/// working on it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecallTask {
    pub message_id: String,
    pub text: String,
    pub time: Option<i64>,
    pub activations: Vec<RecallActivation>,
}

/// A single memory activation: the tool call, its query, and the fragments
/// that were recalled for it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecallActivation {
    pub tool_name: String,
    pub call_id: String,
    pub query: String,
    pub state: Option<String>,
    pub time: Option<i64>,
    /// Resolved retrieval-run identity when the daemon has a matching record.
    pub run_id: Option<String>,
    pub run_status: Option<String>,
    /// Recorded retrieval duration; absent until a matching run is terminal.
    pub total_us: Option<u64>,
    /// Recalled fragments. Populated from the daemon's retrieval run when a
    /// matching run exists; otherwise empty.
    pub fragments: Vec<RecallFragment>,
    /// A short error when the activation's tool result reported failure.
    pub result_error: Option<String>,
}

/// One recalled memory fragment: where it lives and what it says.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecallFragment {
    pub action: Option<String>,
    pub unit_key: String,
    pub resource_id: String,
    pub scope: Option<SourceScope>,
    pub path: String,
    pub heading_path: Vec<String>,
    pub content: String,
    pub final_rank: Option<u64>,
    pub truncated: bool,
}

/// Resolves the bound workspaces (and their project ids) that Activity should
/// read session logs for.
async fn load_bindings(state: &DaemonState) -> Result<Vec<(String, String)>, DaemonError> {
    let rows = sqlx::query(
        "SELECT workspace_root, project_id FROM project_bindings ORDER BY workspace_root",
    )
    .fetch_all(&state.inner.pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let root: String = row.try_get("workspace_root").ok()?;
            let project_id: String = row.try_get("project_id").ok()?;
            Some((root, project_id))
        })
        .collect())
}

pub(super) async fn get_recall_fragment(
    state: &DaemonState,
    request: GetRecallFragmentRequest,
) -> Result<GetRecallFragmentResponse, DaemonError> {
    let bindings = load_bindings(state).await?;
    let (_, project_id) = binding_for_cwd(&request.workspace_root, &bindings).ok_or_else(|| {
        DaemonError::NotFound(format!("Workspace binding {}", request.workspace_root))
    })?;
    let (candidate, content, truncated) = crate::retrieval_history::load_recall_fragment_content(
        state,
        project_id,
        &request.run_id,
        &request.unit_key,
    )
    .await?;
    Ok(GetRecallFragmentResponse {
        fragment: recall_fragment(candidate, content, truncated),
    })
}

fn recall_fragment(
    candidate: RetrievalCandidate,
    content: String,
    truncated: bool,
) -> RecallFragment {
    RecallFragment {
        action: candidate
            .delta_action
            .map(|action| action.as_str().to_owned()),
        unit_key: candidate.unit_key,
        resource_id: candidate.resource_id,
        scope: Some(candidate.scope),
        path: candidate.path,
        heading_path: candidate.heading_path,
        content,
        final_rank: candidate.final_rank,
        truncated,
    }
}

/// Encodes a workspace root into the directory name dsh uses under
/// `~/.dsh/sessions/`. For example `/Volumes/ORICO/workspace/clumsies`
/// becomes `--Volumes-ORICO-workspace-clumsies--`.
fn encode_workspace_dir(root: &str) -> String {
    let trimmed = root.trim_start_matches('/');
    format!("--{}--", trimmed.replace('/', "-").trim_end_matches('-'))
}

fn list_session_files(dir: &Path) -> Result<Vec<PathBuf>, DaemonError> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(DaemonError::Io(error)),
    };
    for entry in entries.flatten() {
        let session_file = entry.path().join("session.jsonl.zstd");
        if session_file.is_file() {
            files.push(session_file);
        }
    }
    Ok(files)
}

fn decompress(path: &Path) -> Result<Vec<u8>, DaemonError> {
    let data = std::fs::read(path)?;
    let data_len = data.len() as u64;
    let mut cursor = std::io::Cursor::new(data);
    let mut output = Vec::new();

    // DSH appends each event as an independent zstd frame. StreamingDecoder
    // intentionally stops after one frame, so recreate it until the archive
    // has been consumed rather than silently returning only the session
    // header from the first frame.
    let mut decoded_frames = 0_usize;
    while cursor.position() < data_len {
        let frame_offset = cursor.position();
        let frame_output_offset = output.len();
        let mut decoder = match ruzstd::decoding::StreamingDecoder::new(&mut cursor) {
            Ok(decoder) => decoder,
            Err(error) if decoded_frames > 0 && error_chain_has_unexpected_eof(&error) => {
                // The file was snapshotted between append writes. Preserve the
                // fully decoded prefix and try the final frame next refresh.
                output.truncate(frame_output_offset);
                break;
            }
            Err(error) => {
                return Err(DaemonError::State {
                    code: "recall_zstd",
                    message: format!(
                        "cannot decode {} at byte {frame_offset}: {error}",
                        path.display()
                    ),
                });
            }
        };
        let read_result = std::io::Read::read_to_end(&mut decoder, &mut output);
        drop(decoder);
        if let Err(error) = read_result {
            if decoded_frames > 0
                && cursor.position() == data_len
                && error_chain_has_unexpected_eof(&error)
            {
                output.truncate(frame_output_offset);
                break;
            }
            return Err(DaemonError::State {
                code: "recall_zstd",
                message: format!(
                    "cannot decode {} at byte {frame_offset}: {error}",
                    path.display()
                ),
            });
        }
        if cursor.position() <= frame_offset {
            return Err(DaemonError::State {
                code: "recall_zstd",
                message: format!(
                    "cannot decode {} at byte {frame_offset}: decoder made no progress",
                    path.display()
                ),
            });
        }
        decoded_frames += 1;
    }

    Ok(output)
}

fn error_chain_has_unexpected_eof(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(source) = current {
        if source
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::UnexpectedEof)
        {
            return true;
        }
        current = source.source();
    }
    false
}

fn is_recall_tool(name: &str) -> bool {
    matches!(name, "mcp__clumsies__memory" | "mcp__clumsies__activate")
}

fn extract_activate(tool_name: &str, args: &str) -> Option<(String, Option<String>)> {
    let value: Value = serde_json::from_str(args).ok()?;
    let object = value.as_object()?;
    // Unified tool: { op: { activate: { query, state } } }
    if let Some(activate) = object
        .get("op")
        .and_then(Value::as_object)
        .and_then(|op| op.get("activate"))
        .and_then(Value::as_object)
        && let Some(query) = activate
            .get("query")
            .and_then(Value::as_str)
            .filter(|query| !query.trim().is_empty())
    {
        let state = activate.get("state").and_then(Value::as_str);
        return Some((query.to_owned(), state.map(str::to_owned)));
    }
    if tool_name != "mcp__clumsies__activate" {
        // The unified `memory` tool has several operations. A top-level query
        // must not turn memory.load/store into a false activation.
        return None;
    }
    // Legacy tool: { query, state }
    let query = object
        .get("query")
        .and_then(Value::as_str)
        .filter(|query| !query.trim().is_empty())?;
    let state = object.get("state").and_then(Value::as_str);
    Some((query.to_owned(), state.map(str::to_owned)))
}

fn message_text(content: &Value) -> Option<String> {
    let parts = content.as_array()?;
    let mut text = String::new();
    for part in parts {
        if let Some(t) = part.get("text").and_then(Value::as_str) {
            text.push_str(t);
        }
    }
    (!text.is_empty()).then_some(text)
}

/// Extracts the tool-result text and whether it reported an error.
fn extract_result(data: &Value) -> Option<(String, bool)> {
    let first = data.get("message")?.get("content")?.as_array()?.first()?;
    let is_error = first
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let inner = first.get("content")?.as_array()?;
    let mut text = String::new();
    for part in inner {
        if let Some(t) = part.get("text").and_then(Value::as_str) {
            text.push_str(t);
        }
    }
    Some((text, is_error))
}

/// Best-effort extraction of fragments already embedded in an activation's
/// tool result. Existing dsh sessions carry no fragments here (they live in
/// the daemon retrieval run), but the shape is stable so future sessions work.
fn extract_fragments(text: &str) -> Vec<RecallFragment> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let Some(fragments) = value.get("fragments").and_then(Value::as_array) else {
        return Vec::new();
    };
    fragments
        .iter()
        .enumerate()
        .filter_map(|(index, fragment)| {
            let path = fragment.get("path").and_then(Value::as_str)?.to_owned();
            let resource_id = fragment
                .get("resource_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let unit_key = fragment
                .get("unit_key")
                .or_else(|| fragment.get("unitKey"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("embedded:{resource_id}:{index}"));
            let action = fragment
                .get("action")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let scope = fragment
                .get("scope")
                .cloned()
                .and_then(|scope| serde_json::from_value(scope).ok());
            let heading_path = fragment
                .get("heading_path")
                .or_else(|| fragment.get("headingPath"))
                .and_then(Value::as_array)
                .map(|headings| {
                    headings
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let content = fragment
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Some(RecallFragment {
                action,
                unit_key,
                resource_id,
                scope,
                path,
                heading_path,
                content,
                final_rank: fragment
                    .get("final_rank")
                    .or_else(|| fragment.get("finalRank"))
                    .and_then(Value::as_u64),
                truncated: fragment
                    .get("truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

fn extract_run_id(text: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(text).ok()?;
    let content = value.get("structuredContent").unwrap_or(&value);
    content
        .get("run_id")
        .or_else(|| content.get("runId"))
        .and_then(Value::as_str)
        .filter(|run_id| !run_id.is_empty())
        .map(str::to_owned)
}

/// Joins an activation against the daemon's retrieval history by project and
/// exact query, returning the recalled fragments when a match exists.
async fn join_retrieval_run(
    state: &DaemonState,
    project_id: &str,
    query: &str,
    requested_run_id: Option<&str>,
) -> Result<Option<(String, String, Option<u64>, Vec<RecallFragment>)>, DaemonError> {
    if project_id.is_empty() {
        return Ok(None);
    }
    let run_id = if let Some(requested_run_id) = requested_run_id {
        // New activation responses carry the run identity. Validate its
        // project boundary before loading details from the shared history DB.
        sqlx::query_scalar(
            "SELECT run_id FROM retrieval_runs
             WHERE run_id = $1 AND project_id = $2 LIMIT 1",
        )
        .bind(requested_run_id)
        .bind(project_id)
        .fetch_optional(&state.inner.pool)
        .await?
    } else {
        // Older logs have only query text. Use that fallback only when it is
        // unique; choosing the newest duplicate silently attaches the wrong
        // retrieval to an earlier session.
        let matches: Vec<String> = sqlx::query_scalar(
            "SELECT run_id FROM retrieval_runs
             WHERE project_id = $1 AND query = $2
             ORDER BY created_at DESC, run_id DESC LIMIT 2",
        )
        .bind(project_id)
        .bind(query)
        .fetch_all(&state.inner.pool)
        .await?;
        (matches.len() == 1).then(|| matches[0].clone())
    };
    let Some(run_id) = run_id else {
        return Ok(None);
    };
    let detail =
        crate::retrieval_history::get_retrieval_run(state, RetrievalRunRequest { run_id }).await?;
    let mut selected = detail
        .candidates
        .into_iter()
        .filter(|candidate| candidate.selected)
        .collect::<Vec<_>>();
    selected.sort_by_key(|candidate| candidate.final_rank.unwrap_or(u64::MAX));
    let fragments = selected
        .into_iter()
        .map(|candidate| {
            let truncated =
                crate::retrieval_history::excerpt_is_truncated(&candidate.evidence_excerpt);
            let content = candidate.evidence_excerpt.clone();
            recall_fragment(candidate, content, truncated)
        })
        .collect();
    Ok(Some((
        detail.run.run_id,
        run_status_str(detail.run.status).to_owned(),
        (detail.run.status != RetrievalRunStatus::Running).then_some(detail.run.latencies.total_us),
        fragments,
    )))
}

fn parse_session_text(text: &str, workspace_root: &str) -> Option<RecallSession> {
    let mut session_id: Option<String> = None;
    let mut title: Option<String> = None;
    let mut created_at: Option<i64> = None;
    let mut cwd: Option<String> = None;

    // Ordered task list plus a lookup for associating tool results to calls.
    let mut tasks: Vec<RecallTask> = Vec::new();
    // Map from callId -> (task index, activation index).
    let mut activation_lookup: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    // Map from callId -> result text, is_error (results can arrive after the
    // call, so we buffer them).
    let mut pending_results: std::collections::HashMap<String, (String, bool)> =
        std::collections::HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            continue;
        };
        let data = event.get("data");

        match event_type {
            "session" => {
                // The session event is flat: id/cwd/createdAt are top-level.
                session_id = event.get("id").and_then(Value::as_str).map(str::to_owned);
                created_at = event.get("createdAt").and_then(Value::as_i64);
                cwd = event.get("cwd").and_then(Value::as_str).map(str::to_owned);
            }
            "session/title" => {
                if let Some(data) = data
                    && let Some(t) = data.get("title").and_then(Value::as_str)
                {
                    title = Some(t.to_owned());
                }
            }
            "user/message" => {
                let Some(data) = data else { continue };
                let is_human = data
                    .get("source")
                    .and_then(|source| source.get("kind"))
                    .and_then(Value::as_str)
                    == Some("user");
                if !is_human {
                    continue;
                }
                if tasks.len() >= MAX_TASKS_PER_SESSION {
                    continue;
                }
                let Some(text_value) = data.get("content").and_then(message_text) else {
                    continue;
                };
                tasks.push(RecallTask {
                    message_id: data
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    text: text_value,
                    time: event.get("time").and_then(Value::as_i64),
                    activations: Vec::new(),
                });
            }
            "tool/call" => {
                let Some(data) = data else { continue };
                let name = data.get("name").and_then(Value::as_str).unwrap_or_default();
                if !is_recall_tool(name) {
                    continue;
                }
                let call_id = data
                    .get("callId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let Some(arguments) = data.get("arguments").and_then(Value::as_str) else {
                    continue;
                };
                let Some((query, state)) = extract_activate(name, arguments) else {
                    continue;
                };
                if tasks.is_empty() {
                    // An activation before any recorded human message; keep a
                    // synthetic task so the activation is never lost.
                    tasks.push(RecallTask {
                        message_id: format!("synthetic-{call_id}"),
                        text: "(no recorded user message)".to_owned(),
                        time: None,
                        activations: Vec::new(),
                    });
                }
                let task_index = tasks.len() - 1;
                if tasks[task_index].activations.len() >= MAX_ACTIVATIONS_PER_TASK {
                    continue;
                }
                let activation_index = tasks[task_index].activations.len();
                tasks[task_index].activations.push(RecallActivation {
                    tool_name: name.to_owned(),
                    call_id: call_id.to_owned(),
                    query,
                    state,
                    time: event.get("time").and_then(Value::as_i64),
                    run_id: None,
                    run_status: None,
                    total_us: None,
                    fragments: Vec::new(),
                    result_error: None,
                });
                activation_lookup.insert(call_id.to_owned(), (task_index, activation_index));
                // Attach a result that arrived before the call.
                if let Some((result_text, is_error)) = pending_results.remove(call_id) {
                    apply_result(
                        &mut tasks,
                        task_index,
                        activation_index,
                        &result_text,
                        is_error,
                    );
                }
            }
            "tool/result" => {
                let Some(data) = data else { continue };
                let Some(call_id) = data
                    .get("message")
                    .and_then(|message| message.get("source"))
                    .and_then(|source| source.get("callId"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Some((result_text, is_error)) = extract_result(data) else {
                    continue;
                };
                match activation_lookup.get(call_id) {
                    Some(&(task_index, activation_index)) => {
                        apply_result(
                            &mut tasks,
                            task_index,
                            activation_index,
                            &result_text,
                            is_error,
                        );
                    }
                    None => {
                        pending_results.insert(call_id.to_owned(), (result_text, is_error));
                    }
                }
            }
            _ => {}
        }
    }

    let session_id = session_id?;
    let root = cwd.unwrap_or_else(|| workspace_root.to_owned());

    Some(RecallSession {
        host: AgentRunHost::Dsh,
        session_id,
        title,
        workspace_root: root,
        created_at,
        tasks,
    })
}

fn parse_session(file: &Path, workspace_root: &str) -> Result<Option<RecallSession>, DaemonError> {
    let bytes = decompress(file)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(parse_session_text(&text, workspace_root))
}

async fn enrich_session(state: &DaemonState, project_id: &str, session: &mut RecallSession) {
    let host = session.host.as_str();
    let session_id = session.session_id.clone();
    for task in &mut session.tasks {
        for activation in &mut task.activations {
            let joined = match join_retrieval_run(
                state,
                project_id,
                &activation.query,
                activation.run_id.as_deref(),
            )
            .await
            {
                Ok(joined) => joined,
                // Retrieval history is independently clearable. A run can
                // disappear between identity validation and detail loading;
                // the session's embedded result is still valid on its own.
                Err(DaemonError::NotFound(_)) => continue,
                Err(error) => {
                    tracing::warn!(
                        host,
                        session_id,
                        call_id = %activation.call_id,
                        "cannot enrich session activation: {error}"
                    );
                    continue;
                }
            };
            if let Some((run_id, run_status, total_us, fragments)) = joined {
                activation.run_id = Some(run_id);
                activation.run_status = Some(run_status);
                activation.total_us = total_us;
                // Retrieval history supplies stable selection metadata and a
                // preview even when a reuse response omitted content. The UI
                // loads the complete frozen chunk when its preview becomes visible.
                if !fragments.is_empty() {
                    activation.fragments = fragments;
                }
            }
        }
    }
}

fn apply_result(
    tasks: &mut [RecallTask],
    task_index: usize,
    activation_index: usize,
    result_text: &str,
    is_error: bool,
) {
    let Some(activation) = tasks
        .get_mut(task_index)
        .and_then(|task| task.activations.get_mut(activation_index))
    else {
        return;
    };
    if is_error {
        activation.result_error = Some(truncate(result_text, 240));
    }
    if activation.run_id.is_none() {
        activation.run_id = extract_run_id(result_text);
    }
    let fragments = extract_fragments(result_text);
    if !fragments.is_empty() {
        activation.fragments = fragments;
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut truncated: String = value.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

fn binding_for_cwd<'a>(
    cwd: &str,
    bindings: &'a [(String, String)],
) -> Option<&'a (String, String)> {
    if cwd.is_empty() {
        return None;
    }
    let candidate = crate::util::canonical_binding_root(cwd);
    bindings
        .iter()
        .filter(|(root, _)| {
            let bound = crate::util::canonical_binding_root(root);
            candidate == bound || candidate.starts_with(&bound)
        })
        // Nested bindings are legal. Attribute a rollout to the most specific
        // workspace rather than whichever row SQLite happened to return first.
        .max_by_key(|(root, _)| {
            crate::util::canonical_binding_root(root)
                .components()
                .count()
        })
}

fn filter_bindings(
    bindings: Vec<(String, String)>,
    workspace_root: Option<&str>,
    project_id: Option<&str>,
) -> Vec<(String, String)> {
    if let Some(root) = workspace_root {
        let canonical = crate::util::canonical_binding_root(root);
        let canonical = canonical.display().to_string();
        let bound_project = bindings
            .iter()
            .find(|(bound, _)| {
                crate::util::canonical_binding_root(bound)
                    == crate::util::canonical_binding_root(&canonical)
            })
            .map(|(_, project)| project.clone())
            .unwrap_or_default();
        return vec![(canonical, bound_project)];
    }
    match project_id {
        Some(project_id) => bindings
            .into_iter()
            .filter(|(_, bound_project)| bound_project == project_id)
            .collect(),
        None => bindings,
    }
}

fn codex_recall_session(session: codex::CodexSession, workspace_root: String) -> RecallSession {
    let codex::CodexSession {
        session_id,
        title,
        cwd: _,
        timestamp: _,
        originator: _,
        created_at,
        tasks,
    } = session;
    RecallSession {
        host: AgentRunHost::Codex,
        session_id,
        title,
        workspace_root,
        created_at,
        tasks: tasks
            .into_iter()
            .map(|task| RecallTask {
                message_id: task.message_id,
                text: task.text,
                // Codex rollout timestamps are RFC 3339 strings. The list is
                // ordered by file mtime, so avoid adding a second date parser
                // just for an optional per-event label.
                time: None,
                activations: task
                    .activations
                    .into_iter()
                    .map(|activation| RecallActivation {
                        tool_name: activation.tool_name,
                        call_id: activation.call_id,
                        query: activation.query,
                        state: activation.state,
                        time: None,
                        run_id: activation.run_id,
                        run_status: None,
                        total_us: None,
                        fragments: activation
                            .fragments
                            .into_iter()
                            .map(|fragment| RecallFragment {
                                action: fragment.action,
                                unit_key: fragment.unit_key,
                                resource_id: fragment.resource_id,
                                scope: fragment.scope.and_then(|scope| {
                                    serde_json::from_value(Value::String(scope)).ok()
                                }),
                                path: fragment.path,
                                heading_path: fragment.heading_path,
                                content: fragment.content,
                                final_rank: None,
                                truncated: false,
                            })
                            .collect(),
                        result_error: activation.result_error,
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn codex_sessions_home(configured_codex_home: Option<&Path>, home: &Path) -> PathBuf {
    configured_codex_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".codex"))
}

pub(super) async fn list_recalls(
    state: &DaemonState,
    request: ListRecallsRequest,
) -> Result<ListRecallsResponse, DaemonError> {
    let limit = request
        .limit
        .map(|limit| limit as usize)
        .unwrap_or(DEFAULT_SESSION_LIMIT)
        .clamp(1, MAX_SESSION_LIMIT);

    let roots = filter_bindings(
        load_bindings(state).await?,
        request.workspace_root.as_deref(),
        request.project_id.as_deref(),
    );

    let home = home_dir()?;
    let sessions_root = home.join(".dsh").join("sessions");

    let mut sessions = Vec::new();
    let mut workspace_roots = Vec::new();
    for (root, project_id) in &roots {
        workspace_roots.push(root.clone());
        let dir = sessions_root.join(encode_workspace_dir(root));
        for file in list_session_files(&dir)? {
            match parse_session(&file, root) {
                Ok(Some(mut session)) => {
                    enrich_session(state, project_id, &mut session).await;
                    sessions.push(session);
                }
                Ok(None) => {}
                Err(error) => {
                    // Active DSH logs can be observed between frame appends.
                    // Keep one transient/corrupt session from blanking every
                    // other provider in the Activity page.
                    tracing::warn!(
                        path = %file.display(),
                        "skipping unreadable DSH session: {error}"
                    );
                }
            }
        }
    }

    let codex_home = codex_sessions_home(state.inner.config.codex_home.as_deref(), &home);
    match codex::load_sessions(&codex_home, limit, |cwd| {
        binding_for_cwd(cwd, &roots).is_some()
    }) {
        Ok(codex_sessions) => {
            for codex_session in codex_sessions {
                let Some((root, project_id)) = binding_for_cwd(&codex_session.cwd, &roots) else {
                    continue;
                };
                let mut session = codex_recall_session(codex_session, root.clone());
                enrich_session(state, project_id, &mut session).await;
                sessions.push(session);
            }
        }
        Err(error) => {
            tracing::warn!("cannot read Codex sessions: {error}");
        }
    }

    sessions.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.session_id.cmp(&a.session_id))
    });
    sessions.truncate(limit);

    Ok(ListRecallsResponse {
        sessions,
        workspace_roots,
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_workspace_directory() {
        assert_eq!(
            encode_workspace_dir("/Volumes/ORICO/workspace/clumsies"),
            "--Volumes-ORICO-workspace-clumsies--"
        );
        assert_eq!(
            encode_workspace_dir("/Users/weiwang/koal-agentos"),
            "--Users-weiwang-koal-agentos--"
        );
    }

    #[test]
    fn codex_sessions_use_dev_config_without_changing_the_stable_default() {
        let home = Path::new("/Users/tester");
        let dev_codex_home = Path::new("/tmp/clumsies-dev/a1b2c3d4e5f6/codex-home");
        assert_eq!(codex_sessions_home(None, home), home.join(".codex"));
        assert_eq!(
            codex_sessions_home(Some(dev_codex_home), home),
            PathBuf::from("/tmp/clumsies-dev/a1b2c3d4e5f6/codex-home")
        );
    }

    #[test]
    fn recognizes_recall_tools() {
        assert!(is_recall_tool("mcp__clumsies__memory"));
        assert!(is_recall_tool("mcp__clumsies__activate"));
        assert!(!is_recall_tool("mcp__clumsies__load"));
        assert!(!is_recall_tool("mcp__clumsies__store"));
        assert!(!is_recall_tool("run_code"));
    }

    #[test]
    fn extracts_unified_activate_query() {
        let (query, state) = extract_activate(
            "mcp__clumsies__memory",
            r#"{"op":{"activate":{"query":"adjust the MCP hybrid retrieval interface","state":"opaque"}}}"#,
        )
        .unwrap();
        assert_eq!(query, "adjust the MCP hybrid retrieval interface");
        assert_eq!(state.as_deref(), Some("opaque"));
    }

    #[test]
    fn extracts_legacy_activate_query() {
        let (query, state) = extract_activate(
            "mcp__clumsies__activate",
            r#"{"query":"writing rules","state":null}"#,
        )
        .unwrap();
        assert_eq!(query, "writing rules");
        assert_eq!(state, None);
    }

    #[test]
    fn does_not_treat_other_unified_memory_operations_as_activation() {
        assert_eq!(
            extract_activate(
                "mcp__clumsies__memory",
                r#"{"op":{"load":{"ids":["memory-1"]}},"query":"not an activation"}"#,
            ),
            None
        );
    }

    #[test]
    fn extracts_stable_run_id_from_activation_result() {
        assert_eq!(
            extract_run_id(r#"{"run_id":"retrieval-1","fragments":[]}"#).as_deref(),
            Some("retrieval-1")
        );
        assert_eq!(
            extract_run_id(r#"{"structuredContent":{"runId":"retrieval-2","fragments":[]}}"#)
                .as_deref(),
            Some("retrieval-2")
        );
    }

    #[test]
    fn extracts_tool_result_text_and_error_flag() {
        let data: Value = serde_json::from_str(
            r#"{"message":{"source":{"kind":"tool","callId":"call_1"},"content":[{"type":"tool-result","toolCallId":"call_1","content":[{"type":"text","text":"boom"}],"isError":true}]}}"#,
        )
        .unwrap();
        let (text, is_error) = extract_result(&data).unwrap();
        assert_eq!(text, "boom");
        assert!(is_error);
    }

    #[test]
    fn parses_a_minimal_session_log() {
        let log = [
            r#"{"type":"session","id":"s1","createdAt":1000,"cwd":"/repo"}"#,
            r#"{"type":"session/title","data":{"title":"tune retrieval"}}"#,
            r#"{"type":"user/message","time":2000,"data":{"id":"m1","role":"user","content":[{"type":"text","text":"帮我调整检索接口"}],"source":{"kind":"user"}}}"#,
            r#"{"type":"tool/call","time":3000,"data":{"callId":"c1","name":"mcp__clumsies__memory","arguments":"{\"op\":{\"activate\":{\"query\":\"adjust MCP retrieval\"}}}"}}"#,
            r#"{"type":"tool/result","time":3001,"data":{"message":{"source":{"kind":"tool","callId":"c1"},"content":[{"type":"tool-result","toolCallId":"c1","content":[{"type":"text","text":"{\"run_id\":\"retrieval-1\",\"fragments\":[]}"}],"isError":false}]}}}"#,
        ]
        .join("\n");
        let session = parse_session_text(&log, "/repo").unwrap();
        assert_eq!(session.session_id, "s1");
        assert_eq!(session.title.as_deref(), Some("tune retrieval"));
        assert_eq!(session.tasks.len(), 1);
        assert_eq!(session.tasks[0].text, "帮我调整检索接口");
        assert_eq!(session.tasks[0].activations.len(), 1);
        assert_eq!(
            session.tasks[0].activations[0].query,
            "adjust MCP retrieval"
        );
        assert_eq!(
            session.tasks[0].activations[0].run_id.as_deref(),
            Some("retrieval-1")
        );
    }

    #[test]
    fn matches_the_most_specific_workspace_binding() {
        let bindings = vec![
            ("/repo".to_owned(), "parent".to_owned()),
            ("/repo/packages/app".to_owned(), "nested".to_owned()),
        ];
        assert_eq!(
            binding_for_cwd("/repo/packages/app/src", &bindings)
                .map(|(_, project_id)| project_id.as_str()),
            Some("nested")
        );
        assert!(binding_for_cwd("/repo-other", &bindings).is_none());
    }

    #[test]
    fn project_filter_includes_every_bound_repository() {
        let bindings = vec![
            ("/repo-a".to_owned(), "project-a".to_owned()),
            ("/repo-b".to_owned(), "project-a".to_owned()),
            ("/repo-c".to_owned(), "project-b".to_owned()),
        ];

        assert_eq!(
            filter_bindings(bindings, None, Some("project-a")),
            vec![
                ("/repo-a".to_owned(), "project-a".to_owned()),
                ("/repo-b".to_owned(), "project-a".to_owned()),
            ]
        );
    }

    #[test]
    fn skips_plugin_injected_user_messages() {
        let log = [
            r#"{"type":"session","id":"s2","createdAt":1,"cwd":"/repo"}"#,
            r#"{"type":"user/message","data":{"id":"p1","role":"user","content":[{"type":"text","text":"system snapshot"}],"source":{"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt"}}}"#,
            r#"{"type":"user/message","data":{"id":"u1","role":"user","content":[{"type":"text","text":"real task"}],"source":{"kind":"user"}}}"#,
        ]
        .join("\n");
        let session = parse_session_text(&log, "/repo").unwrap();
        assert_eq!(session.tasks.len(), 1);
        assert_eq!(session.tasks[0].text, "real task");
    }

    #[test]
    fn decompresses_all_frames_from_an_append_only_dsh_log() {
        use ruzstd::encoding::{CompressionLevel, compress_to_vec};

        // DSH appends each event as its own zstd frame. The session header is
        // therefore in the first frame and human messages are in later ones.
        let header = r#"{"type":"session","id":"s3","createdAt":1,"cwd":"/repo"}
"#;
        let user_message = r#"{"type":"user/message","time":2,"data":{"id":"u1","role":"user","content":[{"type":"text","text":"real task"}],"source":{"kind":"user","rpcId":"rpc-1","clientTimeZone":"Asia/Shanghai"}}}
"#;
        let mut archive = compress_to_vec(header.as_bytes(), CompressionLevel::Uncompressed);
        archive.extend(compress_to_vec(
            user_message.as_bytes(),
            CompressionLevel::Uncompressed,
        ));

        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), archive).unwrap();
        let text = String::from_utf8(decompress(temp.path()).unwrap()).unwrap();
        let session = parse_session_text(&text, "/repo").unwrap();

        assert_eq!(session.tasks.len(), 1);
        assert_eq!(session.tasks[0].text, "real task");
    }

    #[test]
    fn keeps_complete_dsh_frames_when_the_append_tail_is_incomplete() {
        use ruzstd::encoding::{CompressionLevel, compress_to_vec};

        let header = r#"{"type":"session","id":"tail","createdAt":1,"cwd":"/repo"}
"#;
        let user_message = r#"{"type":"user/message","time":2,"data":{"id":"u1","content":[{"type":"text","text":"not committed yet"}],"source":{"kind":"user"}}}
"#;
        let mut archive = compress_to_vec(header.as_bytes(), CompressionLevel::Uncompressed);
        let complete_prefix_len = archive.len();
        archive.extend(compress_to_vec(
            user_message.as_bytes(),
            CompressionLevel::Uncompressed,
        ));
        let nearly_complete = archive.len() - 1;

        // Model snapshots during the header/body write and just before the
        // frame is complete. Neither may expose a partially verified event.
        for snapshot_len in [complete_prefix_len + 8, nearly_complete] {
            let temp = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(temp.path(), &archive[..snapshot_len]).unwrap();
            let text = String::from_utf8(decompress(temp.path()).unwrap()).unwrap();
            let session = parse_session_text(&text, "/repo").unwrap();

            assert_eq!(session.session_id, "tail");
            assert!(session.tasks.is_empty());
        }
    }

    #[test]
    fn decompresses_a_real_dsh_session_when_present() {
        // Best-effort local sanity check: only runs on machines that actually
        // have dsh session logs under ~/.dsh.
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let sessions_root = std::path::Path::new(&home).join(".dsh").join("sessions");
        let Ok(workspaces) = std::fs::read_dir(&sessions_root) else {
            return;
        };
        let mut found = false;
        for workspace in workspaces.flatten() {
            for session in std::fs::read_dir(workspace.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let file = session.path().join("session.jsonl.zstd");
                if !file.is_file() {
                    continue;
                }
                let bytes = decompress(&file).expect("decompress real session");
                let text = String::from_utf8_lossy(&bytes);
                assert!(text.contains("\"type\":\"session\""));
                if let Some(session) = parse_session_text(&text, "/repo") {
                    // A real session must yield a non-empty session id; tasks
                    // may be zero only if the session had no human messages.
                    assert!(!session.session_id.is_empty());
                }
                found = true;
                break;
            }
            if found {
                break;
            }
        }
        if found {
            // Nothing more to assert; decompression already succeeded.
        }
    }
}
