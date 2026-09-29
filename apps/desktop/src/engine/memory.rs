//! Memory API operations; the daemon remains the only session owner.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

pub const ORGANIZATION_MEMORY: &str = "@organization";

fn request(
    method: &str,
    path: &str,
    body: Option<Value>,
    validator: Option<String>,
) -> Result<Value, String> {
    let mut headers = json_headers();
    if method != "GET" {
        headers.insert("Idempotency-Key".into(), uuid::Uuid::new_v4().to_string());
    }
    if let Some(etag) = validator {
        headers.insert("If-Match".into(), etag);
    }
    let response = server(method, path, headers, body.map(|value| value.to_string()))?;
    if !(200..300).contains(&response.status) {
        return Err(server_error(&response));
    }
    serde_json::from_str(&response.body)
        .map_err(|error| format!("Unreadable Memory response: {error}"))
}

pub fn organization_memory() -> Result<Checkout, String> {
    let mut documents = Vec::new();
    let mut path = "/api/v1/org/memories?limit=100".to_owned();
    loop {
        let page = request("GET", &path, None, None)?;
        for meta in page["items"]
            .as_array()
            .ok_or("Missing organization Memory list")?
        {
            let id = meta["memory_id"]
                .as_str()
                .ok_or("Missing Memory identity")?;
            let detail = request("GET", &format!("/api/v1/org/memories/{id}"), None, None)?;
            let text = detail["content"].as_str().ok_or("Missing Memory content")?;
            use sha2::{Digest, Sha256};
            let hash = format!("sha256:{:x}", Sha256::digest(text.as_bytes()));
            if detail["memory"]["memory_id"] != meta["memory_id"]
                || detail["memory"]["path"] != meta["path"]
                || meta["content_hash"].as_str() != Some(&hash)
            {
                return Err("Memory changed while loading; refresh and try again".into());
            }
            documents.push(MemoryDocument {
                resource_id: id.into(),
                published: true,
                path: meta["path"].as_str().ok_or("Missing Memory path")?.into(),
                content: text.into(),
                draft_content: None,
                draft_deleted: false,
                is_directory: meta["is_directory"].as_bool().unwrap_or(false),
                org_owned: true,
            });
        }
        let Some(cursor) = page["page_info"]["next_cursor"].as_str() else {
            break;
        };
        let query: String = reqwest::Url::parse_with_params(
            "http://localhost/",
            [("cursor", cursor), ("limit", "100")],
        )
        .map_err(|e| e.to_string())?
        .query()
        .unwrap()
        .into();
        path = format!("/api/v1/org/memories?{query}");
    }
    documents.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Checkout {
        project_id: ORGANIZATION_MEMORY.into(),
        commit_id: None,
        documents,
    })
}

pub fn change_org_selection(project: &str, resources: &[String], add: bool) -> Result<(), String> {
    // Check current authority as well as the optimistic-concurrency revision.
    let me = request("GET", "/api/v1/me", None, None)?;
    let admin = me["capabilities"]
        .as_array()
        .is_some_and(|values| values.iter().any(|v| v == "admin:write"));
    let manager = me["projects"].as_array().is_some_and(|items| {
        items.iter().any(|p| {
            p["project_id"] == project && matches!(p["role"].as_str(), Some("owner" | "admin"))
        })
    });
    if !admin && !manager {
        return Err("You do not have permission to manage this project's Memory".into());
    }
    let path = format!("/api/v1/projects/{project}/org-selections");
    let selection = request("GET", &path, None, None)?;
    let mut ids: BTreeSet<String> = selection["memories"]
        .as_array()
        .ok_or("Missing organization selections")?
        .iter()
        .filter_map(|m| m["memory_id"].as_str().map(str::to_owned))
        .collect();
    for id in resources {
        if add {
            ids.insert(id.clone());
        } else {
            ids.remove(id);
        }
    }
    let revision = selection["revision"]
        .as_i64()
        .ok_or("Missing selection revision")?;
    request(
        "PUT",
        &path,
        Some(json!({"resource_ids":ids})),
        Some(revision.to_string()),
    )?;
    let response = client()
        .call(DaemonIpcRequest::new(
            "project_retry_sync",
            json!({"project_id":project,"channel":"commits"}),
        ))
        .map_err(|e| e.to_string())?;
    let _: DaemonRetryResponse = response.into_payload().map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReconciliationState {
    pub exists: bool,
    pub resource: Value,
    pub content: Option<DaemonDraftContent>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ReconciliationConflict {
    pub field: String,
    pub kind: String,
    pub base: Option<String>,
    pub current: Option<String>,
    pub draft: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ReconciliationCandidate {
    pub candidate_id: String,
    pub draft_id: String,
    pub draft_version: i64,
    pub current_commit_id: Option<String>,
    pub valid: bool,
    pub current_state: ReconciliationState,
    pub draft_state: ReconciliationState,
    pub proposed_state: Option<ReconciliationState>,
    pub conflicts: Vec<ReconciliationConflict>,
}

pub fn reconciliation_candidate(local_draft: &str) -> Result<ReconciliationCandidate, String> {
    let draft = wait_for_upload(local_draft)?;
    let id = draft.server_draft_id.ok_or("Draft has not uploaded")?;
    let value = request(
        "POST",
        &format!("/api/v1/drafts/{id}/reconciliation-candidates"),
        Some(json!({"expected_draft_version":draft.server_version})),
        None,
    )?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

pub fn apply_reconciliation(
    project: &str,
    candidate: &ReconciliationCandidate,
    state: Option<ReconciliationState>,
) -> Result<(), String> {
    if !candidate.valid {
        return Err("This conflict preview is no longer current. Reopen it.".into());
    }
    request(
        "POST",
        &format!("/api/v1/drafts/{}/rebases", candidate.draft_id),
        Some(
            json!({"candidate_id":candidate.candidate_id,"expected_draft_version":candidate.draft_version,"resolved_state":state}),
        ),
        Some(format!(
            "\"{}\"",
            candidate.current_commit_id.as_deref().unwrap_or("ref-none")
        )),
    )?;
    nudge_drafts(&client(), project)?;
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        if drafts(project)?.iter().any(|d| {
            d.server_draft_id.as_deref() == Some(candidate.draft_id.as_str())
                && d.server_version > candidate.draft_version
        }) {
            return Ok(());
        }
        sleep(Duration::from_millis(250));
    }
    Err("The update was accepted, but local synchronization has not completed. Refresh Memory shortly.".into())
}

/// Use the ZIP library and an atomic destination replacement, never partial output.
pub fn export_memory(entries: &[(String, bool, String)], destination: &Path) -> Result<(), String> {
    let parent = destination.parent().ok_or("Invalid export destination")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    {
        let mut archive = zip::ZipWriter::new(temporary.as_file_mut());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut paths = BTreeSet::new();
        for (path, directory, content) in entries {
            if !crate::memory_paths::valid(path) || !paths.insert(path.clone()) {
                return Err(format!("Invalid or duplicate archive path: {path}"));
            }
            if *directory {
                archive
                    .add_directory(format!("{path}/"), options)
                    .map_err(|e| e.to_string())?;
            } else {
                archive
                    .start_file(path, options)
                    .map_err(|e| e.to_string())?;
                archive
                    .write_all(content.as_bytes())
                    .map_err(|e| e.to_string())?;
            }
        }
        archive.finish().map_err(|e| e.to_string())?;
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(destination).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn propose_org_change(project: &str, resource: &str) -> Result<(), String> {
    if let Some(existing) = drafts(project)?
        .iter()
        .find(|d| d.target_id.as_deref() == Some(resource) && d.scope == DaemonDraftScope::Org)
    {
        return if existing.status == DaemonLocalDraftStatus::Open {
            Ok(())
        } else {
            Err("This organization proposal is already in review".into())
        };
    }
    let head = request("GET", "/api/v1/org/commit-state", None, None)?;
    let base = head["latest"]["commit_id"].as_str().map(str::to_owned);
    let detail = request(
        "GET",
        &format!("/api/v1/org/memories/{resource}"),
        None,
        None,
    )?;
    let after = request("GET", "/api/v1/org/commit-state", None, None)?;
    if head["latest"]["commit_id"] != after["latest"]["commit_id"] {
        return Err("Organization Memory changed; refresh and try again".into());
    }
    draft_operation(&DaemonDraftOperationRequest {
        draft_id: None,
        base_commit_id: base,
        project_id: project.into(),
        scope: DaemonDraftScope::Org,
        resource: DaemonDraftResourceKind::Memory,
        source: Some(DaemonDraftOperationSource::Desktop),
        op: DaemonDraftOperation {
            create: None,
            rename: None,
            delete: None,
            discard: None,
            update: Some(DaemonUpdateDraftOperation::Content(
                DaemonContentDraftUpdate {
                    id: resource.into(),
                    description: None,
                    content: DaemonDraftContent {
                        org_source: None,
                        is_directory: detail["memory"]["is_directory"].as_bool().unwrap_or(false),
                        description: None,
                        content: detail["content"]
                            .as_str()
                            .ok_or("Missing organization content")?
                            .into(),
                    },
                },
            )),
        },
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zip_preserves_unicode_empty_folders_and_rejects_unsafe_paths() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("memory.zip");
        export_memory(
            &[
                ("空目录".into(), true, String::new()),
                ("中文/说明.md".into(), false, "draft contents".into()),
            ],
            &target,
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&target).unwrap()).unwrap();
        assert!(archive.by_name("空目录/").unwrap().is_dir());
        let mut text = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("中文/说明.md").unwrap(), &mut text)
            .unwrap();
        assert_eq!(text, "draft contents");
        drop(archive);
        let original = std::fs::read(&target).unwrap();
        assert!(export_memory(&[("../escape".into(), false, "bad".into())], &target).is_err());
        assert_eq!(original, std::fs::read(&target).unwrap());
    }

    fn sync_checkout(project: &str) -> Checkout {
        client()
            .call(DaemonIpcRequest::new(
                "project_retry_sync",
                json!({"project_id":project,"channel":"commits"}),
            ))
            .unwrap();
        for _ in 0..60 {
            let data = checkout(project).unwrap();
            if data.commit_id.is_some() {
                return data;
            }
            sleep(Duration::from_millis(500));
        }
        panic!("Checkout did not synchronize");
    }
    fn edit(project: &str, checkout: &Checkout, path: &str) -> DocumentEdit {
        let doc = checkout.documents.iter().find(|d| d.path == path).unwrap();
        DocumentEdit {
            project_id: project.into(),
            base_commit_id: checkout.commit_id.clone(),
            draft_id: None,
            resource_id: doc.resource_id.clone(),
            published: doc.published,
            is_directory: doc.is_directory,
            org_owned: false,
            path: doc.path.clone(),
            content: doc.content.clone(),
        }
    }

    /// Only run explicitly against a disposable loopback Dev Instance.
    #[test]
    #[ignore]
    fn local_memory_roundtrip() {
        assert_eq!(std::env::var("CLUMSIES_MEMORY_UI_TEST").as_deref(), Ok("1"));
        let health = client().health().unwrap();
        let url = reqwest::Url::parse(&health.server_url).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        let name = format!("Memory parity test {}", uuid::Uuid::new_v4());
        let project = request("POST", "/api/v1/projects", Some(json!({"name":name})), None)
            .unwrap()["project_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let folder = create_memory_entry(&project, None, "Empty", "", true).unwrap();
        let file = create_document(&project, None, "Nested/README.md", "baseline\n").unwrap();
        let drafts = vec![
            wait_for_upload(&folder.draft_id).unwrap(),
            wait_for_upload(&file.draft_id).unwrap(),
        ];
        let review = request_reviews(
            &drafts,
            "Memory parity baseline",
            "Synthetic test documents",
        )
        .unwrap();
        merge_review(&review).unwrap();
        let baseline = sync_checkout(&project);
        assert!(
            baseline
                .documents
                .iter()
                .any(|d| d.path == "Empty" && d.is_directory)
        );
        let renamed = rename_document(&edit(&project, &baseline, "Empty"), "Moved empty").unwrap();
        wait_for_upload(&renamed.draft_id).unwrap();
        let current = checkout(&project).unwrap();
        assert_eq!(
            current.documents.iter().filter(|d| d.is_directory).count(),
            1
        );
        assert!(
            current
                .documents
                .iter()
                .any(|d| d.path == "Moved empty" && d.is_directory)
        );
        let deletion = delete_document(&edit(&project, &baseline, "Nested/README.md")).unwrap();
        wait_for_upload(&deletion.draft_id).unwrap();
        assert!(
            checkout(&project)
                .unwrap()
                .documents
                .iter()
                .any(|d| d.path == "Nested/README.md" && d.draft_deleted)
        );
        discard_draft(
            &project,
            &deletion.draft_id,
            &edit(&project, &baseline, "Nested/README.md").resource_id,
        )
        .unwrap();
        let draft = create_memory_entry(
            &project,
            baseline.commit_id.as_deref(),
            "Only draft",
            "",
            true,
        )
        .unwrap();
        wait_for_upload(&draft.draft_id).unwrap();
        assert!(
            checkout(&project)
                .unwrap()
                .documents
                .iter()
                .any(|d| d.path == "Only draft" && d.is_directory && !d.published)
        );
        let upstream = create_document(
            &project,
            baseline.commit_id.as_deref(),
            "Upstream.md",
            "new upstream",
        )
        .unwrap();
        let review = request_reviews(
            &[wait_for_upload(&upstream.draft_id).unwrap()],
            "Advance test project",
            "",
        )
        .unwrap();
        merge_review(&review).unwrap();
        for _ in 0..60 {
            let data = checkout(&project).unwrap();
            if data.commit_id != baseline.commit_id {
                break;
            }
            let response = client()
                .call(DaemonIpcRequest::new(
                    "project_retry_sync",
                    json!({"project_id":project,"channel":"commits"}),
                ))
                .unwrap();
            let _: DaemonRetryResponse = response.into_payload().unwrap();
            sleep(Duration::from_millis(250));
        }
        let candidate = reconciliation_candidate(&renamed.draft_id).unwrap();
        assert!(candidate.valid);
        assert!(candidate.conflicts.is_empty());
        apply_reconciliation(&project, &candidate, None).unwrap();
        assert!(
            checkout(&project)
                .unwrap()
                .documents
                .iter()
                .any(|d| d.path == "Moved empty")
        );

        let baseline = checkout(&project).unwrap();
        let mut local = edit(&project, &baseline, "Nested/README.md");
        local.content = "local conflicting text".into();
        let local_draft = store_document(&local).unwrap();
        wait_for_upload(&local_draft.draft_id).unwrap();
        let resource = json!({"scope":"project","id":local.resource_id});
        let remote = request("POST", "/api/v1/drafts", Some(json!({
            "daemon_installation_id":"memory-ui-remote-test", "project_id":project,
            "base_commit_id":baseline.commit_id, "title":"Remote conflict fixture", "resource":resource,
            "operations":[{"action":"update","resource":resource,"content":{"content":"upstream conflicting text"}}]
        })), None).unwrap();
        let remote_review = request("POST", "/api/v1/reviews", Some(json!({
            "title":"Publish remote test edit", "drafts":[{"draft_id":remote["draft"]["draft_id"],"expected_draft_version":remote["draft"]["version"]}]
        })), Some(format!("\"{}\"", baseline.commit_id.as_deref().unwrap()))).unwrap();
        let review: Review = serde_json::from_value(remote_review["review"].clone()).unwrap();
        merge_review(&review).unwrap();
        let candidate = reconciliation_candidate(&local_draft.draft_id).unwrap();
        assert!(!candidate.conflicts.is_empty());
        let mut resolved = candidate.draft_state.clone();
        resolved.content.as_mut().unwrap().content = "manually resolved content".into();
        apply_reconciliation(&project, &candidate, Some(resolved)).unwrap();
        let after = checkout(&project).unwrap();
        assert_eq!(
            after
                .documents
                .iter()
                .find(|d| d.resource_id == local.resource_id)
                .unwrap()
                .draft_content
                .as_deref(),
            Some("manually resolved content")
        );
        eprintln!("Verified conflicting upstream edit and manual resolution");
        if organization_memory().unwrap().documents.is_empty() {
            let operation: DaemonDraftOperationRequest = serde_json::from_value(json!({
                "draft_id":null, "base_commit_id":null, "project_id":project, "scope":"org", "resource":"memory", "source":"desktop",
                "op":{"create":{"path":"UI验收/组织说明.md","content":{"content":"# 组织 Memory\n\n用于测试加入项目、移除和组织修改提案。","is_directory":false}}}
            })).unwrap();
            let created = draft_operation(&operation).unwrap();
            let review = request_reviews(
                &[wait_for_upload(&created.draft_id).unwrap()],
                "Organization Memory UI fixture",
                "",
            )
            .unwrap();
            merge_review(&review).unwrap();
        }
        let organization = organization_memory().unwrap();
        eprintln!(
            "Organization contains {} entries",
            organization.documents.len()
        );
        if let Some(resource) = organization.documents.iter().find(|d| !d.is_directory) {
            change_org_selection(&project, &[resource.resource_id.clone()], true).unwrap();
            propose_org_change(&project, &resource.resource_id).unwrap();
            let draft = super::drafts(&project)
                .unwrap()
                .into_iter()
                .find(|d| {
                    d.scope == DaemonDraftScope::Org
                        && d.target_id.as_deref() == Some(resource.resource_id.as_str())
                })
                .unwrap();
            wait_for_upload(&draft.draft_id).unwrap();
            discard_draft(&project, &draft.draft_id, &resource.resource_id).unwrap();
            change_org_selection(&project, &[resource.resource_id.clone()], false).unwrap();
            eprintln!("Verified organization selection, proposal and removal");
        }
        eprintln!(
            "Verified project {project}: published empty folder, rename projection, deletion color state and new directory draft"
        );
    }
}
