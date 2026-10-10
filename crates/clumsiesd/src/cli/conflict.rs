//! Human conflict choices preserve candidate identity, metadata, and inspection guards.

use clap::Args;
use serde_json::{Value, json};
use std::path::PathBuf;

use super::input;

/// Explicit per-proposal choices for a human Review update.
#[derive(Args, Default)]
pub(super) struct Choices {
    /// Final content as DRAFT_ID=FILE; '-' reads stdin (one proposal only).
    #[arg(long, requires = "resolve", conflicts_with_all = ["file", "edit"])]
    pub(super) content: Vec<String>,
    /// Final path as DRAFT_ID=PATH, required for path conflicts.
    #[arg(long, requires = "resolve", conflicts_with_all = ["file", "edit"])]
    pub(super) path: Vec<String>,
    /// Explicitly delete the named proposal's resource.
    #[arg(long, requires = "resolve", conflicts_with_all = ["file", "edit"])]
    pub(super) delete: Vec<String>,
    /// Explicitly keep the surviving resource in a deletion conflict.
    #[arg(long, requires = "resolve", conflicts_with_all = ["file", "edit"])]
    pub(super) keep: Vec<String>,
}

/// Resolves an ordered batch without sending anything until all choices and edits validate.
///
/// # Errors
/// Rejects stale candidates, unknown/duplicate choices, unsupported conflicts, and remaining
/// markers. Retains every editor file when the final submission fails.
pub(super) fn resolve<T>(
    candidates: &[Value],
    choices: &Choices,
    send: impl FnOnce(Vec<Value>) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    let contents = pairs(&choices.content)?;
    let paths = pairs(&choices.path)?;
    if contents.iter().filter(|(_, path)| path == "-").count() > 1 {
        return Err("Only one proposal may read --content from stdin".into());
    }
    let ids: Vec<_> = candidates
        .iter()
        .filter_map(|c| c["draft_id"].as_str())
        .collect();
    for selected in [&choices.delete, &choices.keep] {
        let mut seen = std::collections::HashSet::new();
        for id in selected {
            if !seen.insert(id) || !ids.contains(&id.as_str()) {
                return Err(format!("Unknown or repeated proposal choice: {id}").into());
            }
        }
    }
    for (id, _) in contents.iter().chain(paths.iter()) {
        if !ids.contains(&id.as_str()) {
            return Err(format!("Unknown proposal choice: {id}").into());
        }
    }
    let mut states = Vec::new();
    let mut initials = Vec::new();
    let mut edits = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let id = candidate["draft_id"]
            .as_str()
            .ok_or("Candidate has no proposal identity")?;
        let path = paths
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, value)| value.as_str());
        let content = contents
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, value)| value.as_str());
        let delete = choices.delete.iter().any(|key| key == id);
        let keep = choices.keep.iter().any(|key| key == id);
        let mut state = prepare(candidate, path, keep, delete)?;
        if state.is_null() {
            if content.is_some() {
                return Err(
                    format!("Clean candidate {id} cannot accept a content override").into(),
                );
            }
        } else if delete {
            if content.is_some() {
                return Err(format!("Deletion for {id} cannot also specify content").into());
            }
        } else {
            let initial = state["content"]["content"]
                .as_str()
                .ok_or("Resolution has no text content")?;
            if let Some(file) = content {
                let text = input::read(&PathBuf::from(file))?;
                set_text(&mut state, candidate, text)?;
            } else if candidate["conflicts"].as_array().is_some_and(|conflicts| {
                conflicts
                    .iter()
                    .any(|conflict| conflict["kind"] == "content")
            }) {
                eprintln!(
                    "{}",
                    super::output::safe_text(&format!(
                        "Edit proposal {id}: {}",
                        state["resource"]["path"].as_str().unwrap_or("(no path)")
                    ))
                );
                initials.push(initial.to_owned());
                edits.push(index);
            } else {
                let text = initial.to_owned();
                set_text(&mut state, candidate, text)?;
            }
        }
        states.push(state);
    }
    input::submit_edits(&initials, |texts| {
        for (index, text) in edits.into_iter().zip(texts) {
            set_text(&mut states[index], &candidates[index], text)?;
        }
        send(states)
    })
}

/// Parses repeatable ID=value choices without silently overriding duplicates.
///
/// # Errors
/// Rejects empty identifiers, empty values, and duplicate proposal keys.
fn pairs(values: &[String]) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let mut result = Vec::new();
    for value in values {
        let (id, text) = value
            .split_once('=')
            .ok_or("Use DRAFT_ID=value for each choice")?;
        if id.is_empty() || text.is_empty() || result.iter().any(|(key, _)| key == id) {
            return Err("Choice identifiers and values must be nonempty and unique".into());
        }
        result.push((id.to_owned(), text.to_owned()));
    }
    Ok(result)
}

/// Builds a complete state while requiring explicit path and existence decisions.
///
/// # Errors
/// Rejects invalid candidates and unresolved non-text dimensions before opening editors.
fn prepare(
    candidate: &Value,
    path: Option<&str>,
    keep: bool,
    delete: bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    if candidate["valid"] != true {
        return Err("Candidate is no longer valid; inspect a new plan before retrying".into());
    }
    if keep && delete || delete && path.is_some() {
        return Err("Choose either keep or delete; deletion cannot also rename".into());
    }
    if candidate["status"] == "clean" {
        if path.is_some() || keep || delete {
            return Err("Clean candidates must be applied without resolution choices".into());
        }
        return Ok(Value::Null);
    }
    if candidate["status"] != "conflicts" {
        return Err("Unknown reconciliation status".into());
    }
    let conflicts = candidate["conflicts"]
        .as_array()
        .ok_or("Candidate has no conflict evidence")?;
    if conflicts.is_empty() {
        return Err("Conflicting candidate has no field-level evidence; inspect a new plan".into());
    }
    let existence = conflicts.iter().any(|c| c["kind"] == "existence");
    if existence && !keep && !delete {
        return Err(format!(
            "Deletion conflict for {}; explicitly use --keep or --delete",
            candidate["draft_id"]
        )
        .into());
    }
    if !existence && keep {
        return Err("--keep is only applicable to a deletion conflict".into());
    }
    if conflicts
        .iter()
        .any(|c| c["kind"] == "path" || c["kind"] == "path_occupied")
        && path.is_none()
        && !delete
    {
        return Err(format!(
            "Path conflict for {}; explicitly specify --path",
            candidate["draft_id"]
        )
        .into());
    }
    if conflicts.iter().any(|c| {
        !matches!(
            c["kind"].as_str(),
            Some("content" | "path" | "path_occupied" | "existence")
        ) || c["kind"] == "content" && c["field"] != "content"
    }) {
        return Err("Unsupported conflict dimension; use the full JSON resolution workflow".into());
    }
    let mut state = if existence && keep {
        if candidate["draft_state"]["exists"] == true {
            candidate["draft_state"].clone()
        } else {
            candidate["current_state"].clone()
        }
    } else {
        candidate["merge_preview"]["state"].clone()
    };
    if !state.is_object() || !state["resource"].is_object() {
        return Err("Candidate has no complete resolution state".into());
    }
    if delete {
        state["exists"] = json!(false);
        state["content"] = Value::Null;
    } else {
        if state["exists"] != true {
            return Err("Resolution does not contain a surviving resource".into());
        }
        if let Some(path) = path {
            state["resource"]["path"] = json!(path);
        }
    }
    Ok(state)
}

/// Changes only the content text and refuses generated conflict markers.
///
/// # Errors
/// Rejects malformed previews or unresolved markers without changing candidate metadata.
fn set_text(
    state: &mut Value,
    candidate: &Value,
    text: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let length = candidate["merge_preview"]["marker_length"]
        .as_u64()
        .filter(|n| *n >= 7)
        .ok_or("Candidate has no valid conflict marker length")?;
    if text.lines().any(|line| {
        let run = line
            .bytes()
            .take_while(|b| matches!(b, b'<' | b'>' | b'|' | b'='))
            .count() as u64;
        run >= length
    }) {
        return Err("Unresolved conflict markers remain; nothing submitted. Edit the retained file and retry with --content".into());
    }
    if !state["content"].is_object() {
        return Err("Resolution has no content metadata".into());
    }
    state["content"]["content"] = json!(text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> Value {
        json!({"draft_id":"d", "candidate_id":"c", "draft_version":3, "valid":true, "status":"conflicts", "conflicts":[{"kind":"content", "field":"content"}], "merge_preview":{"marker_length":8,"state":{"exists":true,"resource":{"id":"m", "scope":"org","path":"a.md"},"content":{"content":"<<<<<<<< ours\nold\n========\nnew\n>>>>>>>> theirs\n","is_directory":false,"org_source":{"id":"source"}}}}})
    }

    #[test]
    fn text_resolution_retains_metadata_and_rejects_markers() {
        let c = candidate();
        let mut state = prepare(&c, None, false, false).unwrap();
        assert!(set_text(&mut state, &c, "<<<<<<<< ours\n".into()).is_err());
        set_text(
            &mut state,
            &c,
            "resolved\n<<<<<<< literal shorter marker\n".into(),
        )
        .unwrap();
        assert_eq!(state["resource"], c["merge_preview"]["state"]["resource"]);
        assert_eq!(state["content"]["org_source"], json!({"id":"source"}));
        assert_eq!(c["draft_version"], 3);
    }

    #[test]
    fn path_and_deletion_require_explicit_choices() {
        let mut c = candidate();
        c["conflicts"] = json!([{"kind":"path","field":"path"}]);
        assert!(prepare(&c, None, false, false).is_err());
        assert_eq!(
            prepare(&c, Some("chosen.md"), false, false).unwrap()["resource"]["path"],
            "chosen.md"
        );
        c["conflicts"] = json!([{"kind":"existence","field":"exists"}]);
        c["current_state"] = c["merge_preview"]["state"].clone();
        c["draft_state"] = json!({"exists":false});
        assert!(prepare(&c, None, false, false).is_err());
        assert_eq!(prepare(&c, None, true, false).unwrap()["exists"], true);
        let deleted = prepare(&c, None, false, true).unwrap();
        assert_eq!(deleted["exists"], false);
        assert!(deleted["content"].is_null());
        assert!(prepare(&c, None, true, true).is_err());
        c["valid"] = json!(false);
        assert!(prepare(&c, None, false, true).is_err());
    }

    #[test]
    fn file_resolution_rejects_unknown_duplicates_and_retains_source() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("resolved.txt");
        std::fs::write(&file, "resolved\n").unwrap();
        let mut choices = Choices {
            content: vec![format!("d={}", file.display())],
            ..Default::default()
        };
        resolve(&[candidate()], &choices, |states| {
            assert_eq!(states[0]["content"]["content"], "resolved\n");
            Err::<(), _>("server rejected".into())
        })
        .unwrap_err();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "resolved\n");
        choices.path.push("unknown=a.md".into());
        assert!(resolve(&[candidate()], &choices, |_| Ok(())).is_err());
        assert!(pairs(&["d=a".into(), "d=b".into()]).is_err());
    }
}
