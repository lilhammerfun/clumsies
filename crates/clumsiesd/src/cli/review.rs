//! Review operations preserve inspected revisions and authoritative Server coordination.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Subcommand;
use clumsiesd::*;
use serde_json::{Value, json};

use super::pagination::{self, PageArgs};
use super::{identifier, print_json, read_json, resolve_project, server};

/// Review workflow, with explicit concurrency inputs for human decisions.
#[derive(Subcommand)]
pub(super) enum ReviewCommand {
    /// List a project's Reviews.
    List {
        /// Project ID or unique name.
        project: String,
        /// Page size and traversal controls.
        #[command(flatten)]
        page: PageArgs,
    },
    /// Inspect full proposals, ordered operations, coordination, and discussion.
    Show {
        /// Review identifier.
        id: String,
    },
    /// Print unified differences against each proposal's immutable base snapshot.
    Diff {
        /// Review identifier.
        id: String,
    },
    /// Submit uploaded local drafts as one Review.
    Create {
        /// Local draft identifiers; order is retained.
        #[arg(required = true)]
        drafts: Vec<String>,
        /// Human summary.
        #[arg(long)]
        title: String,
        /// Explanation of the proposal.
        #[arg(long, default_value = "")]
        description: String,
        /// Candidate choices as an array of ReviewDraftRequest; inspect draft plan first.
        #[arg(long)]
        reconciliations: Option<PathBuf>,
    },
    /// Approve the inspected content (Server policy may publish immediately).
    Approve {
        /// Review identifier.
        id: String,
        /// Revision from show or diff.
        #[arg(long)]
        version: i64,
        /// Optional decision explanation.
        #[arg(long)]
        note: Option<String>,
    },
    /// Reject an inspected proposal and return its drafts to the author.
    Reject {
        /// Review identifier.
        id: String,
        /// Revision from show or diff.
        #[arg(long)]
        version: i64,
        /// Optional explanation.
        #[arg(long)]
        note: Option<String>,
    },
    /// Publish an approved Review against the inspected upstream reference.
    Merge {
        /// Review identifier.
        id: String,
        /// Revision of the approved Review.
        #[arg(long)]
        version: i64,
        /// Current commit identifier from coordination; ref-none for the initial publication.
        #[arg(long)]
        reference: String,
    },
    /// Record discussion on an inspected Review revision.
    Comment {
        /// Review identifier.
        id: String,
        /// Revision from show or diff.
        #[arg(long)]
        version: i64,
        /// Comment text.
        body: String,
    },
    /// Obtain a consistent update plan and an editable request template.
    Plan {
        /// Review identifier.
        id: String,
        /// Inspected Review revision.
        #[arg(long)]
        version: i64,
    },
    /// Apply author-confirmed candidate choices from a plan's request object.
    Update {
        /// Review identifier.
        id: String,
        /// JSON CreateReviewUpdateRequest containing all ordered proposals and inspected version.
        #[arg(long)]
        file: PathBuf,
        /// Upstream reference inspected in the plan; ref-none for an empty reference.
        #[arg(long)]
        reference: String,
    },
}

/// Dispatches human Review actions through daemon-authenticated Server requests.
///
/// # Errors
/// Propagates local upload failures, Server permissions, and stale revision or reference errors.
pub(super) fn run(
    client: &DaemonIpcClient,
    action: ReviewCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        ReviewCommand::List { project, page } => {
            let project = resolve_project(client, &project)?;
            let path = format!("/api/v1/reviews?project_id={}", identifier(&project)?);
            print_json(&pagination::collect(&page, |cursor| {
                server(client, "GET", &page.path(&path, cursor), None, None)
            })?)
        }
        ReviewCommand::Show { id } => print_json(&detail(client, &id)?),
        ReviewCommand::Diff { id } => diff(client, detail(client, &id)?),
        ReviewCommand::Create {
            drafts,
            title,
            description,
            reconciliations,
        } => {
            let choices = reconciliations
                .map(read_json)
                .transpose()?
                .unwrap_or(json!([]));
            let choices = choices
                .as_array()
                .ok_or("Reconciliations must be an array of ReviewDraftRequest")?;
            let mut named = Vec::new();
            let mut reference: Option<String> = None;
            let mut scope_project = None;
            for id in drafts {
                let local = uploaded(client, &id)?;
                let server_id = local
                    .server_draft_id
                    .as_deref()
                    .ok_or("Draft has no server identity")?;
                let remote = server(
                    client,
                    "GET",
                    &format!("/api/v1/drafts/{}", identifier(server_id)?),
                    None,
                    None,
                )?;
                let draft = &remote["draft"];
                let boundary = (local.project_id.clone(), local.scope);
                if scope_project
                    .as_ref()
                    .is_some_and(|existing| existing != &boundary)
                {
                    return Err("A Review must contain drafts from one Project and scope".into());
                }
                scope_project = Some(boundary);
                let head = draft["coordination"]["current_commit_id"]
                    .as_str()
                    .unwrap_or("ref-none")
                    .to_owned();
                if reference.as_ref().is_some_and(|existing| existing != &head) {
                    return Err(
                        "Upstream changed while preparing this Review; inspect the drafts again"
                            .into(),
                    );
                }
                reference = Some(head);
                let choice = choices
                    .iter()
                    .find(|choice| choice["draft_id"] == server_id);
                if draft["coordination"]["freshness"] == "behind" && choice.is_none() {
                    return Err(format!("Draft {id} requires reconciliation; run clumsies draft plan {id}, inspect its candidate, then provide --reconciliations").into());
                }
                named.push(choice.cloned().unwrap_or(
                    json!({"draft_id": server_id, "expected_draft_version": draft["version"]}),
                ));
            }
            if choices.iter().any(|choice| {
                !named
                    .iter()
                    .any(|entry| entry["draft_id"] == choice["draft_id"])
            }) {
                return Err("Reconciliation choices include an unselected draft".into());
            }
            print_json(&server(
                client,
                "POST",
                "/api/v1/reviews",
                Some(json!({"drafts": named, "title": title, "description": description})),
                reference.as_deref(),
            )?)
        }
        ReviewCommand::Approve { id, version, note } => {
            decision(client, &id, version, "approved", note)
        }
        ReviewCommand::Reject { id, version, note } => {
            decision(client, &id, version, "rejected", note)
        }
        ReviewCommand::Merge {
            id,
            version,
            reference,
        } => print_json(&server(
            client,
            "POST",
            &format!("/api/v1/reviews/{}/merges", identifier(&id)?),
            Some(json!({"expected_review_version": version})),
            Some(&reference),
        )?),
        ReviewCommand::Comment { id, version, body } => print_json(&server(
            client,
            "POST",
            &format!("/api/v1/reviews/{}/comments", identifier(&id)?),
            Some(json!({"expected_review_version": version, "body": body})),
            None,
        )?),
        ReviewCommand::Plan { id, version } => {
            let plan = server(
                client,
                "POST",
                &format!("/api/v1/reviews/{}/update-plans", identifier(&id)?),
                Some(json!({"expected_review_version": version})),
                None,
            )?;
            print_json(&update_template(plan)?)
        }
        ReviewCommand::Update {
            id,
            file,
            reference,
        } => {
            let request = read_json(file)?;
            if !request["expected_review_version"].is_i64() || !request["drafts"].is_array() {
                return Err("Use the plan's request object, including expected_review_version and all drafts".into());
            }
            print_json(&server(
                client,
                "POST",
                &format!("/api/v1/reviews/{}/updates", identifier(&id)?),
                Some(request),
                Some(&reference),
            )?)
        }
    }
}

/// Builds an editable resolution template without applying any suggested conflict choices.
///
/// # Errors
/// Rejects missing proposal or revision data instead of submitting an incomplete batch.
fn update_template(plan: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let candidates = plan["candidates"]
        .as_array()
        .ok_or("Invalid update candidates")?;
    let entries = plan["detail"]["drafts"]
        .as_array()
        .ok_or("Invalid Review proposal set")?;
    let mut drafts = Vec::new();
    for entry in entries {
        let draft = &entry["draft"];
        let id = draft["draft_id"].as_str().ok_or("Missing draft identity")?;
        let candidate = candidates
            .iter()
            .find(|candidate| candidate["draft_id"] == id);
        drafts.push(json!({
            "draft_id": id,
            "expected_draft_version": candidate.map(|candidate| &candidate["draft_version"]).unwrap_or(&draft["version"]),
            "candidate_id": candidate.map(|candidate| &candidate["candidate_id"]),
            // Conflict choices remain explicit: the operator copies and edits merge_preview.state.
            "resolved_state": null,
        }));
    }
    let version = plan["detail"]["review"]["version"]
        .as_i64()
        .ok_or("Missing Review revision")?;
    Ok(json!({"request": {"expected_review_version": version, "drafts": drafts}, "plan": plan}))
}

/// Records the exact inspected revision without fetching and substituting a newer one.
///
/// # Errors
/// Propagates Server permission, lifecycle, or version conflicts.
fn decision(
    client: &DaemonIpcClient,
    id: &str,
    version: i64,
    decision: &str,
    note: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    print_json(&server(
        client,
        "POST",
        &format!("/api/v1/reviews/{}/decisions", identifier(id)?),
        Some(json!({"expected_review_version": version, "decision": decision, "body": note})),
        None,
    )?)
}

/// Gets one authoritative Review with its ordered proposals and discussion.
///
/// # Errors
/// Propagates malformed identifiers or Server read failures.
fn detail(client: &DaemonIpcClient, id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    server(
        client,
        "GET",
        &format!("/api/v1/reviews/{}", identifier(id)?),
        None,
        None,
    )
}

/// Waits finitely for durable upload before a Server Review names the draft.
///
/// # Errors
/// Returns operation failure or timeout without discarding any local draft.
pub(super) fn uploaded(
    client: &DaemonIpcClient,
    id: &str,
) -> Result<DaemonDraftSummary, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut nudged = false;
    loop {
        let detail = client.get_draft(id)?;
        if detail.draft.failed_operation_count > 0 {
            return Err(format!(
                "Draft upload failed; inspect clumsies draft show {id} before retrying"
            )
            .into());
        }
        if detail.draft.pending_operation_count == 0 && detail.draft.server_draft_id.is_some() {
            return Ok(detail.draft);
        }
        if Instant::now() >= deadline {
            return Err("Draft is not synchronized yet; local work is retained".into());
        }
        if !nudged {
            client
                .call(DaemonIpcRequest::new(
                    "project_retry_sync",
                    serde_json::to_value(DaemonProjectSyncRetryRequest {
                        project_id: detail.draft.project_id,
                        channel: SyncRetryChannel::Drafts,
                    })?,
                ))?
                .into_payload::<DaemonRetryResponse>()?;
            nudged = true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Renders each proposal from the immutable ancestor through its ordered operations.
///
/// # Errors
/// Refuses incomplete snapshots or unsupported operation shapes instead of showing a false diff.
fn diff(client: &DaemonIpcClient, detail: Value) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "Review {} version {} status {}\nReference {}",
        detail["review"]["review_id"],
        detail["review"]["version"],
        detail["review"]["status"],
        detail["review"]["coordination"]["current_commit_id"]
            .as_str()
            .unwrap_or("ref-none")
    );
    let entries = detail["drafts"]
        .as_array()
        .ok_or("Review has no ordered proposal set")?;
    for entry in entries {
        let draft = &entry["draft"];
        let base = if let Some(commit) = draft["base_commit_id"].as_str() {
            server(
                client,
                "GET",
                &format!("/api/v1/commits/{}", identifier(commit)?),
                None,
                None,
            )?
        } else {
            json!({"tree": {"entries": []}, "blobs": []})
        };
        let (before_path, before, after_path, after) =
            proposal_text(draft, &entry["operations"], &base)?;
        // Text alone hides directory, provenance, and description changes.
        println!(
            "Resource metadata and ordered operations:\n{}",
            serde_json::to_string_pretty(&json!({
                "base_resource": base_resource(&draft["resource"], &base)?,
                "operations": entry["operations"],
            }))?
        );
        println!(
            "{}",
            similar::TextDiff::from_lines(&before, &after)
                .unified_diff()
                .header(&before_path, &after_path)
        );
        if before == after && before_path != after_path {
            println!("rename from {before_path}\nrename to {after_path}");
        }
    }
    Ok(())
}

/// Finds the resource only within the proposal's authority scope.
///
/// # Errors
/// Rejects snapshots without a resource tree.
fn base_resource<'a>(
    resource: &Value,
    base: &'a Value,
) -> Result<Option<&'a Value>, Box<dyn std::error::Error>> {
    let entries = base["tree"]["entries"]
        .as_array()
        .ok_or("Snapshot has no resource entries")?;
    Ok(entries.iter().find(|entry| {
        (!resource["id"].is_null()
            && entry["id"] == resource["id"]
            && entry["scope"] == resource["scope"])
            || (resource["id"].is_null()
                && !resource["path"].is_null()
                && entry["path"] == resource["path"]
                && entry["scope"] == resource["scope"])
    }))
}

/// Applies create/update/rename/delete ordering to snapshot text for one resource.
///
/// # Errors
/// Rejects missing base blobs or unknown operations to prevent misleading review evidence.
fn proposal_text(
    draft: &Value,
    operations: &Value,
    base: &Value,
) -> Result<(String, String, String, String), Box<dyn std::error::Error>> {
    let target = base_resource(&draft["resource"], base)?;
    let before_path = target
        .and_then(|entry| entry["path"].as_str())
        .unwrap_or("/dev/null")
        .to_owned();
    let before = if let Some(target) = target {
        base["blobs"]
            .as_array()
            .ok_or("Snapshot has no blobs")?
            .iter()
            .find(|blob| blob["blob_id"] == target["blob_id"])
            .and_then(|blob| blob["content"].as_str())
            .ok_or("Snapshot is missing the target content")?
            .to_owned()
    } else {
        String::new()
    };
    let mut after = before.clone();
    let mut after_path = before_path.clone();
    let mut exists = target.is_some();
    for operation in operations
        .as_array()
        .ok_or("Draft operations are missing")?
    {
        match operation["action"].as_str() {
            Some("create") => {
                if exists {
                    return Err(
                        "Create targets an existing resource; inspect full Review JSON".into(),
                    );
                }
                exists = true;
                after_path = operation["resource"]["path"]
                    .as_str()
                    .ok_or("Create has no path")?
                    .to_owned();
                after = operation["content"]["content"]
                    .as_str()
                    .ok_or("Create has no content")?
                    .to_owned();
            }
            Some("update") => {
                if !exists {
                    return Err("Update is missing its immutable base resource".into());
                }
                after = operation["content"]["content"]
                    .as_str()
                    .ok_or("Update has no content")?
                    .to_owned();
            }
            Some("rename") => {
                if !exists {
                    return Err("Rename is missing its immutable base resource".into());
                }
                after_path = operation["new_path"]
                    .as_str()
                    .ok_or("Rename has no destination")?
                    .to_owned();
            }
            Some("delete") => {
                if !exists {
                    return Err("Delete is missing its immutable base resource".into());
                }
                exists = false;
                after.clear();
                after_path = "/dev/null".to_owned();
            }
            _ => return Err("Unsupported Review operation; inspect full Review JSON".into()),
        }
    }
    Ok((before_path, before, after_path, after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_retains_base_and_ordered_update_then_rename() {
        let draft = json!({"resource": {"id": "mem_1", "path": "old.md", "scope": "project"}});
        let base = json!({"tree": {"entries": [{"id": "mem_1", "scope": "project", "path": "old.md", "blob_id": "b"}]}, "blobs": [{"blob_id": "b", "content": "old\n"}]});
        let operations = json!([{"action": "update", "content": {"content": "new\n"}}, {"action": "rename", "new_path": "new.md"}]);
        assert_eq!(
            proposal_text(&draft, &operations, &base).unwrap(),
            (
                "old.md".into(),
                "old\n".into(),
                "new.md".into(),
                "new\n".into()
            )
        );
    }

    #[test]
    fn diff_refuses_missing_or_wrong_scope_base() {
        let draft = json!({"resource": {"id": "mem_1", "scope": "project"}});
        let base = json!({"tree": {"entries": [{"id": "mem_1", "scope": "org", "path": "wrong.md", "blob_id": "b"}]}, "blobs": [{"blob_id": "b", "content": "private"}]});
        for action in ["update", "rename", "delete"] {
            assert!(proposal_text(&draft, &json!([{"action": action, "content": {"content": "new"}, "new_path": "new.md"}]), &base).is_err());
        }
    }

    #[test]
    fn conflict_template_never_silently_picks_a_side() {
        let plan = json!({"detail": {"review": {"version": 7}, "drafts": [{"draft": {"draft_id": "d", "version": 3}}]}, "candidates": [{"draft_id": "d", "draft_version": 3, "candidate_id": "c", "status": "conflicts", "merge_preview": {"state": {"content": "suggestion"}}}]});
        let result = update_template(plan).unwrap();
        assert_eq!(result["request"]["expected_review_version"], 7);
        assert_eq!(result["request"]["drafts"][0]["candidate_id"], "c");
        assert!(result["request"]["drafts"][0]["resolved_state"].is_null());
    }
}
