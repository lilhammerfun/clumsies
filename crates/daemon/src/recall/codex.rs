//! Reads Codex rollout metadata separately from selected session task bodies.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

#[cfg(test)]
use std::convert::Infallible;

const SYNTHETIC_TASK_TEXT: &str = "(no recorded user message)";
const MAX_HEADER_LINES: usize = 16;
const MAX_HEADER_BYTES: u64 = 2 * 1024 * 1024;

/// Whether a rollout still lives in the active session tree or was archived.
/// Active files always win when both trees contain the same session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RolloutSource {
    Live,
    Archived,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RolloutFile {
    pub(super) path: PathBuf,
    pub(super) source: RolloutSource,
    /// Last modification time, in milliseconds since the Unix epoch.
    pub(super) created_at: Option<i64>,
}

/// The small, recall-specific slice of a Codex rollout consumed by the UI.
/// This deliberately is not a generic transcript model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodexSession {
    pub(super) session_id: String,
    pub(super) title: Option<String>,
    pub(super) cwd: String,
    /// The timestamp recorded by `session_meta`, retained in its source form.
    pub(super) timestamp: Option<String>,
    pub(super) originator: Option<String>,
    /// File mtime supplied by the caller, in milliseconds since the epoch.
    pub(super) created_at: Option<i64>,
    pub(super) tasks: Vec<CodexTask>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodexTask {
    pub(super) message_id: String,
    pub(super) text: String,
    pub(super) timestamp: Option<String>,
    pub(super) activations: Vec<CodexActivation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodexActivation {
    pub(super) tool_name: String,
    pub(super) call_id: String,
    pub(super) query: String,
    pub(super) state: Option<String>,
    pub(super) timestamp: Option<String>,
    pub(super) run_id: Option<String>,
    pub(super) fragments: Vec<CodexFragment>,
    pub(super) result_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodexFragment {
    pub(super) action: Option<String>,
    pub(super) unit_key: String,
    pub(super) resource_id: String,
    pub(super) scope: Option<String>,
    pub(super) path: String,
    pub(super) heading_path: Vec<String>,
    pub(super) content: String,
}

/// Finds active `sessions/YYYY/MM/DD/rollout-*.jsonl` files and archived
/// rollouts below a Codex home directory. Discovery is recursive so older and
/// future directory layouts continue to work. A duplicate filename in the
/// archive is discarded in favor of the active copy.
pub(super) fn discover_rollouts(codex_home: &Path) -> io::Result<Vec<RolloutFile>> {
    let mut live = collect_rollouts(&codex_home.join("sessions"), RolloutSource::Live)?;
    let mut archived = collect_rollouts(
        &codex_home.join("archived_sessions"),
        RolloutSource::Archived,
    )?;
    live.sort_by(|a, b| a.path.cmp(&b.path));
    archived.sort_by(|a, b| a.path.cmp(&b.path));

    let mut seen = HashSet::<OsString>::new();
    let mut files = Vec::with_capacity(live.len() + archived.len());
    for file in live.into_iter().chain(archived) {
        let Some(key) = file.path.file_name().map(OsString::from) else {
            continue;
        };
        if seen.insert(key) {
            files.push(file);
        }
    }
    Ok(files)
}

/// Reads Codex's append-only title index. Later records for the same session
/// replace earlier records, matching the current title shown by Codex.
pub(super) fn read_session_titles(index_path: &Path) -> io::Result<HashMap<String, String>> {
    let bytes = match fs::read(index_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(error),
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut titles = HashMap::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        let title = entry
            .get("thread_name")
            .or_else(|| entry.get("title"))
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty());
        if let Some(title) = title {
            titles.insert(id.to_owned(), title.to_owned());
        }
    }
    Ok(titles)
}

/// Bounded metadata used to filter before reading a session's tasks.
#[derive(Debug)]
pub(super) struct RolloutHeader {
    /// Host-local session identity.
    pub(super) session_id: String,
    /// Workspace recorded by the host, resolved against the most specific binding.
    pub(super) cwd: String,
    /// Subagents do not form independent human Activity rows.
    pub(super) is_subagent: bool,
}

/// A deduplicated log file eligible for a lightweight Activity summary.
#[derive(Debug)]
pub(super) struct RolloutCandidate {
    /// Discovered path, source, and modification timestamp.
    pub(super) file: RolloutFile,
    /// Identity and binding metadata from the bounded header.
    pub(super) header: RolloutHeader,
}

/// Discovers matching sessions using bounded headers, without reading task bodies.
///
/// # Errors
/// Returns directory discovery errors; individually unreadable logs are skipped.
pub(super) fn list_candidates(
    codex_home: &Path,
    workspace_matches: impl Fn(&str) -> bool,
) -> io::Result<Vec<RolloutCandidate>> {
    let files = discover_rollouts(codex_home)?;
    let mut candidates = HashMap::<String, RolloutCandidate>::new();

    for file in files {
        // A malformed, unreadable, or concurrently-moved rollout must not
        // prevent other sessions from appearing in the diagnostic panel.
        let Ok(Some(header)) = read_rollout_header(&file.path) else {
            continue;
        };
        if header.is_subagent || !workspace_matches(&header.cwd) {
            continue;
        }

        let session_id = header.session_id.clone();
        let candidate = RolloutCandidate { file, header };
        match candidates.get_mut(&session_id) {
            None => {
                candidates.insert(session_id, candidate);
            }
            Some(existing) if should_replace_file(&existing.file, &candidate.file) => {
                *existing = candidate;
            }
            Some(_) => {}
        }
    }

    let mut candidates: Vec<_> = candidates.into_values().collect();
    candidates.sort_by(|a, b| {
        b.file
            .created_at
            .cmp(&a.file.created_at)
            .then_with(|| b.header.session_id.cmp(&a.header.session_id))
            .then_with(|| b.file.path.cmp(&a.file.path))
    });
    Ok(candidates)
}

#[cfg(test)]
fn load_sessions(
    codex_home: &Path,
    limit: usize,
    workspace_matches: impl Fn(&str) -> bool,
) -> io::Result<Vec<CodexSession>> {
    let titles = read_session_titles(&codex_home.join("session_index.jsonl"))?;
    let mut sessions = Vec::new();
    for candidate in list_candidates(codex_home, workspace_matches)?
        .into_iter()
        .take(limit)
    {
        let file = candidate.file;
        let Ok(Some(mut session)) = parse_rollout_file(&file.path, None, file.created_at) else {
            continue;
        };
        if session.session_id != candidate.header.session_id {
            continue;
        }
        session.title = titles.get(&session.session_id).cloned();
        sessions.push(session);
    }
    Ok(sessions)
}

/// Reads a bounded first-prompt preview when the host has no indexed title.
///
/// # Errors
/// Returns file read errors; callers may still display the session identity.
pub(super) fn read_title_preview(path: &Path) -> io::Result<Option<String>> {
    let reader = BufReader::new(fs::File::open(path)?.take(MAX_HEADER_BYTES));
    let session = parse_rollout_lines(reader.lines().take(32), None, None)?;
    Ok(session.and_then(|s| s.tasks.first().map(|t| t.text.chars().take(160).collect())))
}

/// Parses the recall-relevant events from one rollout. Malformed and unrelated
/// JSONL records are ignored so a partially-written live rollout remains
/// readable.
#[cfg(test)]
fn parse_rollout(
    text: &str,
    title: Option<&str>,
    file_mtime_millis: Option<i64>,
) -> Option<CodexSession> {
    match parse_rollout_lines(
        text.lines().map(Ok::<_, Infallible>),
        title,
        file_mtime_millis,
    ) {
        Ok(session) => session,
        Err(never) => match never {},
    }
}

/// Reads the task bodies of one selected session.
///
/// # Errors
/// Returns file open and streaming read errors.
pub(super) fn parse_rollout_file(
    path: &Path,
    title: Option<&str>,
    file_mtime_millis: Option<i64>,
) -> io::Result<Option<CodexSession>> {
    let file = fs::File::open(path)?;
    parse_rollout_lines(BufReader::new(file).lines(), title, file_mtime_millis)
}

fn parse_rollout_lines<I, S, E>(
    lines: I,
    title: Option<&str>,
    file_mtime_millis: Option<i64>,
) -> Result<Option<CodexSession>, E>
where
    I: IntoIterator<Item = Result<S, E>>,
    S: AsRef<str>,
{
    let mut session_id = None;
    let mut cwd = None;
    let mut session_timestamp = None;
    let mut originator = None;
    let mut tasks = Vec::<CodexTask>::new();
    let mut current_task = None;
    let mut activation_number = 0_usize;
    let mut has_structured_user_messages = false;

    for line in lines {
        let line = line?;
        let Ok(record) = serde_json::from_str::<Value>(line.as_ref()) else {
            continue;
        };
        match record.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let Some(payload) = record.get("payload") else {
                    continue;
                };
                assign_string(&mut session_id, payload.get("id"));
                assign_string(&mut cwd, payload.get("cwd"));
                assign_string(&mut session_timestamp, payload.get("timestamp"));
                assign_string(&mut originator, payload.get("originator"));
            }
            Some("event_msg") => {
                let Some(payload) = record.get("payload") else {
                    continue;
                };
                match payload.get("type").and_then(Value::as_str) {
                    Some("user_message") => {
                        if has_structured_user_messages {
                            continue;
                        }
                        let Some(message) = payload
                            .get("message")
                            .and_then(Value::as_str)
                            .filter(|message| !message.trim().is_empty())
                        else {
                            continue;
                        };
                        tasks.push(CodexTask {
                            message_id: format!("codex-task-{}", tasks.len() + 1),
                            text: message.to_owned(),
                            timestamp: record_timestamp(&record),
                            activations: Vec::new(),
                        });
                        current_task = Some(tasks.len() - 1);
                    }
                    Some(event @ ("mcp_tool_call_end" | "item_completed")) => {
                        let (invocation, call_id, result, error) = match event {
                            "mcp_tool_call_end" => {
                                let Some(invocation) = payload.get("invocation") else {
                                    continue;
                                };
                                (
                                    invocation,
                                    payload.get("call_id"),
                                    payload.get("result"),
                                    None,
                                )
                            }
                            _ => {
                                let Some(item) = payload.get("item").filter(|item| {
                                    item.get("type").and_then(Value::as_str) == Some("McpToolCall")
                                }) else {
                                    continue;
                                };
                                (item, item.get("id"), item.get("result"), item.get("error"))
                            }
                        };
                        let server = invocation
                            .get("server")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let tool = invocation
                            .get("tool")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if server != "clumsies" {
                            continue;
                        }
                        let Some((query, state)) = invocation
                            .get("arguments")
                            .and_then(|arguments| extract_activation_arguments(tool, arguments))
                        else {
                            continue;
                        };

                        activation_number += 1;
                        let call_id = call_id
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("codex-activation-{activation_number}"));
                        if current_task.is_none() && tasks.is_empty() {
                            tasks.push(CodexTask {
                                message_id: format!("synthetic-{call_id}"),
                                text: SYNTHETIC_TASK_TEXT.to_owned(),
                                timestamp: None,
                                activations: Vec::new(),
                            });
                            current_task = Some(0);
                        }
                        let Some(task_index) = current_task else {
                            continue;
                        };

                        let result = result.unwrap_or(&Value::Null);
                        let activation = CodexActivation {
                            tool_name: tool.to_owned(),
                            call_id,
                            query,
                            state,
                            timestamp: record_timestamp(&record),
                            run_id: extract_run_id(result),
                            fragments: extract_fragments(result),
                            result_error: extract_result_error(result)
                                .or_else(|| error.map(|error| truncate(&display_json(error), 240))),
                        };
                        if let Some(task) = tasks.get_mut(task_index) {
                            task.activations.push(activation);
                        }
                    }
                    _ => {}
                }
            }
            Some("response_item") => {
                let Some(payload) = record.get("payload") else {
                    continue;
                };
                let Some((message_id, message)) = extract_user_prompt(payload) else {
                    continue;
                };
                has_structured_user_messages = true;
                tasks.push(CodexTask {
                    message_id,
                    text: message,
                    timestamp: record_timestamp(&record),
                    activations: Vec::new(),
                });
                current_task = Some(tasks.len() - 1);
            }
            _ => {}
        }
    }

    let Some(session_id) = session_id else {
        return Ok(None);
    };
    Ok(Some(CodexSession {
        session_id,
        title: title
            .filter(|title| !title.trim().is_empty())
            .map(str::to_owned),
        cwd: cwd.unwrap_or_default(),
        timestamp: session_timestamp,
        originator,
        created_at: file_mtime_millis,
        tasks,
    }))
}

fn read_rollout_header(path: &Path) -> io::Result<Option<RolloutHeader>> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file).take(MAX_HEADER_BYTES);
    let mut line = String::new();

    for _ in 0..MAX_HEADER_LINES {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(payload) = record.get("payload") else {
            return Ok(None);
        };
        let Some(session_id) = payload
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return Ok(None);
        };
        let cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Ok(Some(RolloutHeader {
            session_id: session_id.to_owned(),
            cwd: cwd.to_owned(),
            is_subagent: payload
                .get("source")
                .and_then(|source| source.get("subagent"))
                .is_some(),
        }));
    }

    Ok(None)
}

fn collect_rollouts(dir: &Path, source: RolloutSource) -> io::Result<Vec<RolloutFile>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut pending = vec![dir.to_owned()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() && is_rollout_file(&path) {
                files.push(RolloutFile {
                    created_at: file_mtime_millis(&path).ok().flatten(),
                    path,
                    source,
                });
            }
        }
    }
    Ok(files)
}

fn is_rollout_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

fn file_mtime_millis(path: &Path) -> io::Result<Option<i64>> {
    let modified = fs::metadata(path)?.modified()?;
    let Ok(duration) = modified.duration_since(UNIX_EPOCH) else {
        return Ok(None);
    };
    Ok(Some(duration.as_millis().min(i64::MAX as u128) as i64))
}

fn should_replace_file(old: &RolloutFile, new: &RolloutFile) -> bool {
    match (old.source, new.source) {
        (RolloutSource::Archived, RolloutSource::Live) => true,
        (RolloutSource::Live, RolloutSource::Archived) => false,
        _ => new
            .created_at
            .cmp(&old.created_at)
            .then_with(|| new.path.cmp(&old.path))
            .is_gt(),
    }
}

fn assign_string(target: &mut Option<String>, value: Option<&Value>) {
    if let Some(value) = value.and_then(Value::as_str) {
        *target = Some(value.to_owned());
    }
}

fn record_timestamp(record: &Value) -> Option<String> {
    record
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn extract_user_prompt(payload: &Value) -> Option<(String, String)> {
    if payload.get("type").and_then(Value::as_str) != Some("message")
        || payload.get("role").and_then(Value::as_str) != Some("user")
    {
        return None;
    }
    let content = payload.get("content").and_then(Value::as_array)?;
    let kinds = payload
        .get("internal_chat_message_metadata_passthrough")
        .and_then(|metadata| metadata.get("content_item_kinds"))
        .and_then(Value::as_array)?;
    let text = content
        .iter()
        .zip(kinds)
        .filter(|(_, kind)| kind.as_str() == Some("user.text"))
        .filter_map(|(item, _)| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        return None;
    }
    let message_id = payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?
        .to_owned();
    Some((message_id, text))
}

fn extract_activation_arguments(tool: &str, value: &Value) -> Option<(String, Option<String>)> {
    if let Some(encoded) = value.as_str() {
        let decoded = serde_json::from_str::<Value>(encoded).ok()?;
        return extract_activation_arguments(tool, &decoded);
    }
    let object = value.as_object()?;
    let unified_activate = object
        .get("op")
        .and_then(Value::as_object)
        .and_then(|op| op.get("activate"))
        .and_then(Value::as_object);
    let activate = match tool {
        // The unified MCP tool contains load/store/activate operations. Only
        // the activate variant belongs in Sessions.
        "memory" => unified_activate?,
        // Older Codex rollouts recorded a dedicated tool name. Accept either
        // its direct arguments or the unified envelope for compatibility.
        "memory/activate" | "activate" => unified_activate.unwrap_or(object),
        _ => return None,
    };
    let query = activate
        .get("query")
        .and_then(Value::as_str)
        .filter(|query| !query.is_empty())?
        .to_owned();
    let state = activate
        .get("state")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some((query, state))
}

fn extract_run_id(result: &Value) -> Option<String> {
    result
        .get("Ok")
        .unwrap_or(result)
        .get("structuredContent")
        .and_then(|content| content.get("run_id").or_else(|| content.get("runId")))
        .and_then(Value::as_str)
        .filter(|run_id| !run_id.is_empty())
        .map(str::to_owned)
}

fn extract_fragments(result: &Value) -> Vec<CodexFragment> {
    let Some(fragments) = result
        .get("Ok")
        .unwrap_or(result)
        .get("structuredContent")
        .and_then(|content| content.get("fragments"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    fragments
        .iter()
        .enumerate()
        .filter_map(|(index, fragment)| {
            let object = fragment.as_object()?;
            let action = object
                .get("action")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let resource_id = object
                .get("resource_id")
                .or_else(|| object.get("resourceId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let unit_key = object
                .get("unit_key")
                .or_else(|| object.get("unitKey"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("embedded:{resource_id}:{index}"));
            let scope = object
                .get("scope")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let path = object
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let heading_path = object
                .get("heading_path")
                .or_else(|| object.get("headingPath"))
                .and_then(Value::as_array)
                .map(|headings| {
                    headings
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let content = object
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            (!resource_id.is_empty() || !path.is_empty() || !content.is_empty()).then_some(
                CodexFragment {
                    action,
                    unit_key,
                    resource_id,
                    scope,
                    path,
                    heading_path,
                    content,
                },
            )
        })
        .collect()
}

fn extract_result_error(result: &Value) -> Option<String> {
    if let Some(error) = result.get("Err") {
        return Some(truncate(&display_json(error), 240));
    }
    let ok = result.get("Ok").unwrap_or(result);
    if !ok.get("isError").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let text = ok
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    Some(truncate(
        if text.is_empty() {
            "activation failed"
        } else {
            &text
        },
        240,
    ))
}

fn display_json(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| value.to_string())
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut truncated: String = value.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_memory_activation_and_fragments() {
        let rollout = [
            r#"{"timestamp":"2026-08-19T08:00:00.000Z","type":"session_meta","payload":{"id":"session-1","timestamp":"2026-08-19T07:59:00.000Z","cwd":"/repo","originator":"codex_desktop"}}"#,
            "not json",
            r#"{"timestamp":"2026-08-19T08:01:00.000Z","type":"response_item","payload":{"type":"message","id":"prompt-1","role":"user","content":[{"type":"input_text","text":"Show the recalled memory"}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]}}}"#,
            r#"{"timestamp":"2026-08-19T08:01:01.000Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"call-1","server":"clumsies","tool":"memory","arguments":{"op":{"activate":{"query":"recall parser","state":"opaque"}}},"status":"completed","result":{"structuredContent":{"run_id":"run-1","fragments":[{"action":"add","resource_id":"rule-1","path":"rules/parser.md","content":"Keep only recall data."}]},"isError":false}}}}"#,
            r#"{"timestamp":"2026-08-19T08:01:02.000Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"load-1","server":"clumsies","tool":"memory","arguments":{"op":{"load":{"ids":["rule-1"]}}},"status":"completed","result":{"structuredContent":{},"isError":false}}}}"#,
            r#"{"timestamp":"2026-08-19T08:01:03.000Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"call-2","server":"clumsies","tool":"memory","arguments":{"op":{"activate":{"query":"retry parser"}}},"status":"failed","error":{"message":"connection closed"}}}}"#,
        ]
        .join("\n");

        let session = parse_rollout(&rollout, Some("Parser work"), Some(1234)).unwrap();
        assert_eq!(session.session_id, "session-1");
        assert_eq!(session.title.as_deref(), Some("Parser work"));
        assert_eq!(session.cwd, "/repo");
        assert_eq!(
            session.timestamp.as_deref(),
            Some("2026-08-19T07:59:00.000Z")
        );
        assert_eq!(session.originator.as_deref(), Some("codex_desktop"));
        assert_eq!(session.created_at, Some(1234));
        assert_eq!(session.tasks.len(), 1);
        assert_eq!(session.tasks[0].text, "Show the recalled memory");
        assert_eq!(session.tasks[0].activations.len(), 2);

        let activation = &session.tasks[0].activations[0];
        assert_eq!(activation.call_id, "call-1");
        assert_eq!(activation.tool_name, "memory");
        assert_eq!(activation.query, "recall parser");
        assert_eq!(activation.state.as_deref(), Some("opaque"));
        assert_eq!(activation.run_id.as_deref(), Some("run-1"));
        assert_eq!(activation.fragments.len(), 1);
        assert_eq!(activation.fragments[0].resource_id, "rule-1");
        assert_eq!(activation.fragments[0].path, "rules/parser.md");
        assert_eq!(activation.fragments[0].content, "Keep only recall data.");
        assert_eq!(
            session.tasks[0].activations[1].result_error.as_deref(),
            Some("connection closed")
        );
    }

    #[test]
    fn creates_one_synthetic_task_for_activations_before_a_user_message() {
        let rollout = [
            r#"{"type":"session_meta","payload":{"id":"session-2","cwd":"/repo"}}"#.to_owned(),
            activation_line("call-1", "first"),
            activation_line("call-2", "second"),
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"real task"}}"#
                .to_owned(),
            activation_line("call-3", "third"),
        ]
        .join("\n");

        let session = parse_rollout(&rollout, None, None).unwrap();
        assert_eq!(session.tasks.len(), 2);
        assert_eq!(session.tasks[0].text, SYNTHETIC_TASK_TEXT);
        assert_eq!(session.tasks[0].activations.len(), 2);
        assert_eq!(session.tasks[1].text, "real task");
        assert_eq!(session.tasks[1].activations.len(), 1);
    }

    #[test]
    fn ignores_non_clumsies_and_non_activation_mcp_calls() {
        let rollout = [
            r#"{"type":"session_meta","payload":{"id":"session-3","cwd":"/repo"}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"task"}}"#,
            r#"{"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"c1","invocation":{"server":"other","tool":"memory/activate","arguments":{"query":"no"}},"result":{"Ok":{"structuredContent":{"fragments":[]}}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"c2","invocation":{"server":"clumsies","tool":"memory","arguments":{"op":{"load":{"path":"rules.md"}}}},"result":{"Ok":{"structuredContent":{}}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"c3","invocation":{"server":"clumsies","tool":"memory","arguments":{"op":{"store":{"content":"not recall"}}}},"result":{"Ok":{"structuredContent":{}}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"c4","invocation":{"server":"clumsies","tool":"memory","arguments":{"query":"missing activate op"}},"result":{"Ok":{"structuredContent":{}}}}}"#,
        ]
        .join("\n");

        let session = parse_rollout(&rollout, None, None).unwrap();
        assert!(session.tasks[0].activations.is_empty());
    }

    #[test]
    fn accepts_dedicated_and_legacy_activate_tools() {
        let rollout = [
            r#"{"type":"session_meta","payload":{"id":"legacy","cwd":"/repo"}}"#,
            r#"{"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"dedicated-call","invocation":{"server":"clumsies","tool":"memory/activate","arguments":{"query":"dedicated query"}},"result":{"Ok":{"structuredContent":{"runId":"run-camel","fragments":[]}}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"legacy-call","invocation":{"server":"clumsies","tool":"activate","arguments":"{\"query\":\"legacy query\"}"},"result":{"Ok":{"structuredContent":{"fragments":[]}}}}}"#,
        ]
        .join("\n");

        let session = parse_rollout(&rollout, None, None).unwrap();
        assert_eq!(session.tasks[0].activations.len(), 2);
        assert_eq!(session.tasks[0].activations[0].query, "dedicated query");
        assert_eq!(
            session.tasks[0].activations[0].run_id.as_deref(),
            Some("run-camel")
        );
        assert_eq!(session.tasks[0].activations[1].query, "legacy query");
    }

    #[test]
    fn preserves_tasks_and_activations_for_pagination() {
        let mut lines = vec![
            r#"{"type":"session_meta","payload":{"id":"bounded","cwd":"/repo"}}"#.to_owned(),
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"first"}}"#
                .to_owned(),
        ];
        for index in 0..=100 {
            lines.push(activation_line(&format!("call-{index}"), "bounded query"));
        }
        for index in 1..500 {
            lines.push(format!(
                r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"task {index}"}}}}"#
            ));
        }
        lines.push(
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"overflow"}}"#
                .to_owned(),
        );
        lines.push(activation_line("overflow-call", "last task activation"));

        let session = parse_rollout(&lines.join("\n"), None, None).unwrap();
        assert_eq!(session.tasks.len(), 501);
        assert_eq!(session.tasks[0].activations.len(), 101);
        assert_eq!(
            session
                .tasks
                .iter()
                .map(|task| task.activations.len())
                .sum::<usize>(),
            102
        );
    }

    #[test]
    fn loads_only_direct_user_prompts_from_current_codex_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions/2026/08/28/rollout-root.jsonl");
        write_rollout(
            &root,
            &[
                r#"{"type":"session_meta","payload":{"id":"root","cwd":"/repo","source":"vscode"}}"#,
                r#"{"type":"response_item","payload":{"type":"message","id":"context","role":"user","content":[{"type":"input_text","text":"<environment_context>ambient</environment_context>"}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":["environments.environment_context"]}}}"#,
                r#"{"type":"response_item","payload":{"type":"message","id":"prompt-1","role":"user","content":[{"type":"input_text","text":"first prompt"}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]}}}"#,
                r#"{"type":"response_item","payload":{"type":"message","id":"prompt-2","role":"user","content":[{"type":"input_text","text":"second prompt"}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]}}}"#,
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"The following is the Codex agent history..."}}"#,
            ]
            .join("\n"),
        );
        set_mtime(&root, 100);

        let guardian = temp
            .path()
            .join("sessions/2026/08/28/rollout-guardian.jsonl");
        write_rollout(
            &guardian,
            &[
                r#"{"type":"session_meta","payload":{"id":"guardian","cwd":"/repo","source":{"subagent":{"other":"guardian"}}}}"#,
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"The following is the Codex agent history..."}}"#,
            ]
            .join("\n"),
        );
        set_mtime(&guardian, 200);

        let sessions = load_sessions(temp.path(), 1, |_| true).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "root");
        assert_eq!(
            sessions[0]
                .tasks
                .iter()
                .map(|task| task.text.as_str())
                .collect::<Vec<_>>(),
            ["first prompt", "second prompt"]
        );
    }

    #[test]
    fn latest_session_index_title_wins() {
        let temp = tempfile::tempdir().unwrap();
        let index = temp.path().join("session_index.jsonl");
        fs::write(
            &index,
            [
                r#"{"id":"s1","thread_name":"Old title"}"#,
                "bad json",
                r#"{"id":"s1","thread_name":"Current title"}"#,
                r#"{"id":"s2","title":"Fallback key"}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let titles = read_session_titles(&index).unwrap();
        assert_eq!(titles.get("s1").map(String::as_str), Some("Current title"));
        assert_eq!(titles.get("s2").map(String::as_str), Some("Fallback key"));
    }

    #[test]
    fn discovers_nested_live_and_archived_rollouts_with_live_priority() {
        let temp = tempfile::tempdir().unwrap();
        let duplicate = "rollout-2026-08-19T00-00-00-duplicate.jsonl";
        write_rollout(&temp.path().join("sessions/2026/08/19").join(duplicate), "");
        write_rollout(&temp.path().join("archived_sessions").join(duplicate), "");
        write_rollout(
            &temp
                .path()
                .join("archived_sessions/old")
                .join("rollout-archived-only.jsonl"),
            "",
        );
        write_rollout(
            &temp.path().join("sessions/2026/08/19/not-a-rollout.txt"),
            "",
        );

        let files = discover_rollouts(temp.path()).unwrap();
        assert_eq!(files.len(), 2);
        let duplicate = files
            .iter()
            .find(|file| file.path.file_name().unwrap() == duplicate)
            .unwrap();
        assert_eq!(duplicate.source, RolloutSource::Live);
    }

    #[test]
    fn load_sessions_deduplicates_by_meta_id_and_prefers_live() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("session_index.jsonl"),
            r#"{"id":"same-session","thread_name":"Indexed title"}"#,
        )
        .unwrap();

        let old_live = temp
            .path()
            .join("sessions/2026/08/19/rollout-live-old.jsonl");
        write_rollout(&old_live, &minimal_rollout("same-session", "live task"));
        set_mtime(&old_live, 100);

        let new_live = temp
            .path()
            .join("sessions/2026/08/19/rollout-live-new.jsonl");
        write_rollout(&new_live, &minimal_rollout("same-session", "new live task"));
        set_mtime(&new_live, 200);

        let archived = temp.path().join("archived_sessions/rollout-archived.jsonl");
        write_rollout(&archived, &minimal_rollout("same-session", "archived task"));
        // Archive is newer, but a live copy still has precedence.
        set_mtime(&archived, 300);

        let sessions = load_sessions(temp.path(), 10, |_| true).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("Indexed title"));
        assert_eq!(sessions[0].tasks[0].text, "new live task");
    }

    #[test]
    fn load_sessions_filters_headers_before_sorting_and_limit() {
        let temp = tempfile::tempdir().unwrap();
        let older_wanted = temp
            .path()
            .join("sessions/2026/08/19/rollout-wanted-old.jsonl");
        write_rollout(
            &older_wanted,
            &minimal_rollout_in("wanted-old", "/wanted", "older wanted"),
        );
        set_mtime(&older_wanted, 100);

        let newer_wanted = temp
            .path()
            .join("sessions/2026/08/19/rollout-wanted-new.jsonl");
        write_rollout(
            &newer_wanted,
            &minimal_rollout_in("wanted-new", "/wanted", "newer wanted"),
        );
        set_mtime(&newer_wanted, 200);

        let newest_other = temp.path().join("sessions/2026/08/19/rollout-other.jsonl");
        write_rollout(
            &newest_other,
            &minimal_rollout_in("other", "/other", "must be filtered"),
        );
        set_mtime(&newest_other, 300);

        // A bad file is isolated during the header pass.
        let malformed = temp
            .path()
            .join("sessions/2026/08/19/rollout-malformed.jsonl");
        write_rollout(&malformed, "not json\nstill not json");
        set_mtime(&malformed, 400);

        let sessions = load_sessions(temp.path(), 1, |cwd| cwd == "/wanted").unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "wanted-new");
        assert_eq!(sessions[0].tasks[0].text, "newer wanted");
    }

    fn activation_line(call_id: &str, query: &str) -> String {
        format!(
            r#"{{"type":"event_msg","payload":{{"type":"mcp_tool_call_end","call_id":"{call_id}","invocation":{{"server":"clumsies","tool":"memory/activate","arguments":{{"query":"{query}"}}}},"result":{{"Ok":{{"structuredContent":{{"fragments":[]}}}}}}}}}}"#
        )
    }

    fn minimal_rollout(session_id: &str, task: &str) -> String {
        minimal_rollout_in(session_id, "/repo", task)
    }

    fn minimal_rollout_in(session_id: &str, cwd: &str, task: &str) -> String {
        [
            format!(r#"{{"type":"session_meta","payload":{{"id":"{session_id}","cwd":"{cwd}"}}}}"#),
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"{task}"}}}}"#
            ),
        ]
        .join("\n")
    }

    fn write_rollout(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn set_mtime(path: &Path, millis: u64) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        let modified = UNIX_EPOCH + std::time::Duration::from_millis(millis);
        file.set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }
}
