//! Count Memory participation in completed, explicitly delimited local Codex turns.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;

use super::{binding_for_cwd, codex, codex_sessions_home, load_bindings};
use crate::dashboard::DashboardRetrievalRequest;
use crate::{DaemonError, DaemonState};

/// Coverage is limited to locally observable completed Codex main-agent turns.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentUsageStatistics {
    /// Daily completed turns, including turns without any Memory operation.
    pub days: Vec<AgentUsageDay>,
    /// Files whose bodies could not be read or parsed completely.
    pub unreadable_sessions: usize,
    /// Readable session files without recognized native run boundaries.
    pub unsupported_sessions: usize,
}

/// One calendar day's observed terminal turns, attributed by start time.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentUsageDay {
    /// Local midnight, as Unix seconds.
    pub date: i64,
    /// Turns with at least one activate, load or store invocation, even if it failed.
    pub with_memory: usize,
    /// Turns with no observed Clumsies Memory invocation.
    pub without_memory: usize,
}

/// Minimal per-turn state; never includes prompts, arguments or returned memory.
#[derive(Clone, Debug, Default, Serialize)]
struct Run {
    /// Host-provided start timestamp in RFC 3339 form.
    at: String,
    /// Workspace at the turn's boundary.
    cwd: String,
    /// Whether a terminal event was observed.
    completed: bool,
    /// Host timestamp of completion or interruption.
    finished_at: String,
    /// Whether a recognized Memory operation was invoked.
    used: bool,
}

/// Parsed run metadata for one stable snapshot of a log file.
#[derive(Clone, Default)]
struct ParsedUsage {
    /// Deduplicated host turns.
    runs: Vec<Run>,
    /// Whether native boundaries were present, even for ongoing turns.
    supported: bool,
    /// Whether complete records were malformed or tool completion coverage was ambiguous.
    incomplete: bool,
}

/// Cached summaries keyed by file metadata; contains no conversation text.
#[derive(Default)]
pub(super) struct UsageCache {
    /// Reparse changed logs; discard summaries when logs are removed or leave scope.
    files: HashMap<PathBuf, (SystemTime, u64, ParsedUsage)>,
}

/// Recognizes only Clumsies Memory operations, not arbitrary tools named memory.
fn is_memory_call(server: Option<&str>, name: &str) -> bool {
    matches!(name, "mcp__clumsies__memory" | "mcp__clumsies__activate")
        || (server == Some("clumsies") && matches!(name, "memory" | "activate"))
}

/// Reads native boundaries and tool events; user messages never create denominator rows.
///
/// # Errors
/// Propagates file read errors. Malformed records mark the entire session incomplete.
fn parse(
    reader: impl BufRead,
    expected_session: &str,
    initial_cwd: &str,
) -> Result<ParsedUsage, std::io::Error> {
    let mut runs = BTreeMap::<String, Run>::new();
    let mut current: Option<String> = None;
    let mut result = ParsedUsage::default();
    let mut session_matches = false;
    for line in reader.lines() {
        let line = line?;
        let record: Value = match serde_json::from_str(&line) {
            Ok(record) => record,
            Err(_) => {
                result.incomplete = true;
                continue;
            }
        };
        let payload = &record["payload"];
        let timestamp = record["timestamp"].as_str().unwrap_or_default();
        match record["type"].as_str() {
            Some("session_meta") => {
                session_matches = payload["id"].as_str() == Some(expected_session)
            }
            Some("event_msg") => match payload["type"].as_str() {
                Some("task_started") => {
                    let Some(id) = payload["turn_id"].as_str() else {
                        result.incomplete = true;
                        continue;
                    };
                    result.supported = true;
                    current = Some(id.to_owned());
                    runs.entry(id.to_owned()).or_insert(Run {
                        at: timestamp.into(),
                        cwd: initial_cwd.into(),
                        ..Default::default()
                    });
                }
                Some("task_complete" | "turn_aborted") => {
                    let id = payload["turn_id"].as_str().or(current.as_deref());
                    if let Some(run) = id.and_then(|id| runs.get_mut(id)) {
                        run.completed = true;
                        run.finished_at = timestamp.to_owned();
                    } else {
                        result.incomplete = true;
                    }
                    if id == current.as_deref() {
                        current = None;
                    }
                }
                Some("mcp_tool_call_begin" | "mcp_tool_call_end" | "item_completed") => {
                    let item = if payload["type"] == "item_completed" {
                        if payload["item"]["type"] != "McpToolCall" {
                            continue;
                        }
                        &payload["item"]
                    } else {
                        &payload["invocation"]
                    };
                    if is_memory_call(
                        item["server"].as_str(),
                        item["tool"].as_str().unwrap_or_default(),
                    ) {
                        if let Some(run) = payload["turn_id"]
                            .as_str()
                            .or(current.as_deref())
                            .and_then(|id| runs.get_mut(id))
                        {
                            run.used = true;
                        } else {
                            result.incomplete = true;
                        }
                    }
                }
                _ => {}
            },
            Some("turn_context") => {
                if let Some(run) = payload["turn_id"].as_str().and_then(|id| runs.get_mut(id))
                    && let Some(cwd) = payload["cwd"].as_str()
                {
                    run.cwd = cwd.to_owned();
                }
            }
            Some("response_item")
                if payload["type"] == "function_call"
                    && is_memory_call(None, payload["name"].as_str().unwrap_or_default()) =>
            {
                if let Some(run) = current.as_ref().and_then(|id| runs.get_mut(id)) {
                    run.used = true;
                } else {
                    result.incomplete = true;
                }
            }
            _ => {}
        }
    }
    result.incomplete |= !session_matches;
    result.runs = runs.into_values().collect();
    Ok(result)
}

/// Returns scoped local usage; unreadable/unsupported sessions never become unused turns.
///
/// # Errors
/// Propagates binding, discovery, worker and SQL failures instead of reporting false zeros.
pub(crate) async fn statistics(
    state: &DaemonState,
    request: &DashboardRetrievalRequest,
) -> Result<AgentUsageStatistics, DaemonError> {
    let bindings = load_bindings(state).await?;
    let projects = request.project_ids.clone();
    let home = crate::util::home_dir()?;
    let codex_home = codex_sessions_home(state.inner.config.codex_home.as_deref(), &home);
    let cache = state.inner.recall_cache.lock().await.usage.clone();
    let (runs, mut statistics) = tokio::task::spawn_blocking(move || -> Result<_, DaemonError> {
        let candidates = codex::list_candidates(&codex_home, |cwd| {
            binding_for_cwd(cwd, &bindings).is_some_and(|(_, id)| projects.contains(id))
        })?;
        let mut cache = cache
            .lock()
            .map_err(|_| DaemonError::InvalidRequest("Agent usage cache lock failed".into()))?;
        let paths: HashSet<_> = candidates.iter().map(|c| c.file.path.clone()).collect();
        cache.files.retain(|path, _| paths.contains(path));
        let mut statistics = AgentUsageStatistics::default();
        let mut runs = Vec::new();
        for candidate in candidates {
            let path = &candidate.file.path;
            let parsed = (|| -> Result<_, std::io::Error> {
                let metadata = fs::metadata(path)?;
                let modified = metadata.modified()?;
                if let Some((old_time, old_size, parsed)) = cache.files.get(path)
                    && *old_time == modified
                    && *old_size == metadata.len()
                {
                    return Ok(parsed.clone());
                }
                // ponytail: reparse only changed logs; use byte-offset indexing if active large logs become costly.
                let parsed = parse(
                    BufReader::new(fs::File::open(path)?),
                    &candidate.header.session_id,
                    &candidate.header.cwd,
                )?;
                cache
                    .files
                    .insert(path.clone(), (modified, metadata.len(), parsed.clone()));
                Ok(parsed)
            })();
            let parsed = match parsed {
                Ok(parsed) if !parsed.incomplete => parsed,
                _ => {
                    statistics.unreadable_sessions += 1;
                    continue;
                }
            };
            if !parsed.supported {
                statistics.unsupported_sessions += 1;
                continue;
            }
            runs.extend(parsed.runs.into_iter().filter(|run| {
                run.completed
                    && binding_for_cwd(&run.cwd, &bindings)
                        .is_some_and(|(_, id)| projects.contains(id))
            }));
        }
        Ok((runs, statistics))
    })
    .await
    .map_err(|error| {
        DaemonError::InvalidRequest(format!("Agent usage worker failed: {error}"))
    })??;
    // SQLite already parses the RFC 3339 timestamps used throughout local telemetry.
    let rows = sqlx::query(
        "WITH runs AS (SELECT CAST(strftime('%s',json_extract(value,'$.at')) AS INTEGER) AS at,
          json_extract(value,'$.used') AS used,
          CAST(strftime('%s',json_extract(value,'$.finished_at')) AS INTEGER) AS finished FROM json_each($1))
         SELECT at, used FROM runs WHERE at BETWEEN $2 AND $3 AND finished <= $3",
    )
    .bind(serde_json::to_string(&runs)?)
    .bind(request.day_bounds[0])
    .bind(request.generated_at)
    .fetch_all(&state.inner.pool)
    .await?;
    let mut days = BTreeMap::new();
    for row in rows {
        let at: i64 = row.try_get("at")?;
        let date = request.day_bounds[request.day_bounds.partition_point(|bound| *bound <= at) - 1];
        let day = days.entry(date).or_insert(AgentUsageDay {
            date,
            with_memory: 0,
            without_memory: 0,
        });
        if row.try_get::<bool, _>("used")? {
            day.with_memory += 1;
        } else {
            day.without_memory += 1;
        }
    }
    statistics.days = days.into_values().collect();
    Ok(statistics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_turns_include_zero_usage_and_deduplicate_calls_and_end_events() {
        let log = r#"{"type":"session_meta","payload":{"id":"s"}}
{"type":"event_msg","timestamp":"2026-09-28T01:00:00Z","payload":{"type":"task_started","turn_id":"one"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"steering is not a new run"}]}}
{"type":"event_msg","payload":{"type":"task_complete","turn_id":"one"}}
{"type":"event_msg","timestamp":"2026-09-28T02:00:00Z","payload":{"type":"task_started","turn_id":"two"}}
{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"McpToolCall","server":"clumsies","tool":"memory","arguments":{"op":{"load":{"ids":["a"]}}}}}}
{"type":"event_msg","payload":{"type":"mcp_tool_call_end","invocation":{"server":"clumsies","tool":"memory","arguments":{"op":{"store":{}}}}}}
{"type":"event_msg","payload":{"type":"task_complete","turn_id":"two"}}
{"type":"event_msg","payload":{"type":"task_complete","turn_id":"two"}}
{"type":"event_msg","timestamp":"2026-09-28T03:00:00Z","payload":{"type":"task_started","turn_id":"ongoing"}}"#;
        let parsed = parse(log.as_bytes(), "s", "/repo").unwrap();
        assert!(parsed.supported);
        assert!(!parsed.incomplete);
        let finished: Vec<_> = parsed.runs.iter().filter(|run| run.completed).collect();
        assert_eq!(finished.len(), 2);
        assert_eq!(finished.iter().filter(|run| run.used).count(), 1);
        assert!(!is_memory_call(Some("other"), "memory"));
        assert!(is_memory_call(None, "mcp__clumsies__memory"));
        // Invalid or missing arguments still represent an attempted Memory call.
        assert!(is_memory_call(Some("clumsies"), "memory"));
    }

    #[test]
    fn missing_boundaries_or_malformed_logs_are_not_unused_runs() {
        let legacy = r#"{"type":"session_meta","payload":{"id":"s"}}
{"type":"event_msg","payload":{"type":"user_message","message":"hello"}}"#;
        let parsed = parse(legacy.as_bytes(), "s", "/repo").unwrap();
        assert!(!parsed.supported);
        assert!(parsed.runs.is_empty());
        let parsed = parse(format!("{legacy}\ninvalid").as_bytes(), "s", "/repo").unwrap();
        assert!(parsed.incomplete);
        assert!(
            parse(legacy.as_bytes(), "replaced-session", "/repo")
                .unwrap()
                .incomplete
        );
    }
    #[tokio::test]
    async fn aggregation_scopes_dates_and_refreshes_cached_files() {
        let temp = tempfile::tempdir().unwrap();
        let (state, directory, root) = super::super::paging::tests::fixture(&temp).await;
        let header =
            serde_json::json!({"type":"session_meta","payload":{"id":"session","cwd":root}})
                .to_string();
        let first = format!("{header}\n")
            + r#"{"type":"event_msg","timestamp":"2026-09-28T01:00:00Z","payload":{"type":"task_started","turn_id":"a"}}
{"type":"response_item","payload":{"type":"function_call","name":"mcp__clumsies__memory","arguments":"{\"op\":{\"load\":{}}}"}}
{"type":"event_msg","timestamp":"2026-09-28T01:01:00Z","payload":{"type":"task_complete","turn_id":"a"}}
"#;
        let file = directory.join("rollout-one.jsonl");
        fs::write(&file, &first).unwrap();
        let today: i64 = sqlx::query_scalar("SELECT unixepoch('2026-09-28T00:00:00Z')")
            .fetch_one(&state.inner.pool)
            .await
            .unwrap();
        let mut request = DashboardRetrievalRequest {
            project_ids: vec!["project".into()],
            resources: vec![],
            day_bounds: (-6..=1).map(|offset| today + offset * 86400).collect(),
            recency_starts: vec![today - 6 * 86400, today - 29 * 86400, today - 89 * 86400],
            generated_at: today + 7200,
        };
        let result = statistics(&state, &request).await.unwrap();
        assert_eq!(result.days.len(), 1);
        assert_eq!(
            (result.days[0].with_memory, result.days[0].without_memory),
            (1, 0)
        );
        let next = first
            + r#"{"type":"event_msg","timestamp":"2026-09-28T01:02:00Z","payload":{"type":"task_started","turn_id":"b"}}
{"type":"event_msg","timestamp":"2026-09-28T01:03:00Z","payload":{"type":"task_complete","turn_id":"b"}}
{"type":"event_msg","timestamp":"2026-09-29T01:00:00Z","payload":{"type":"task_started","turn_id":"future"}}
{"type":"event_msg","timestamp":"2026-09-29T01:01:00Z","payload":{"type":"task_complete","turn_id":"future"}}
"#;
        fs::write(&file, next).unwrap();
        let result = statistics(&state, &request).await.unwrap();
        assert_eq!(
            (result.days[0].with_memory, result.days[0].without_memory),
            (1, 1)
        );
        request.project_ids = vec!["other".into()];
        assert!(statistics(&state, &request).await.unwrap().days.is_empty());
        request.project_ids = vec!["project".into()];
        fs::write(&file, format!("{header}\nmalformed")).unwrap();
        let result = statistics(&state, &request).await.unwrap();
        assert!(result.days.is_empty());
        assert_eq!(result.unreadable_sessions, 1);
        state.inner.pool.close().await;
    }
}
