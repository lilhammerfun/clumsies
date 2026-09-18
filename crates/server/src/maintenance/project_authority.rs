//! Explicit, plan-hash-guarded maintenance of legacy project authority data.

use crate::app::commit::service::{
    advance_project_ref, create_project_commit, current_project_ref,
};
use crate::app::draft::dto::{
    CreateDraftRequest, DraftEventType, DraftOperationAction, DraftOperationInput,
    DraftResourceContent, DraftResourceRef,
};
use crate::app::draft::service::{create_draft_in_tx as create_draft, insert_draft_event};
use crate::app::memory::dto::ResourceScope;
use crate::app::memory::model::{
    content_hash, insert_materialization_path, materialization_output_path, validate_resource_path,
};
use crate::app::memory::service::lock_org_draft_selection_coordination;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::types::Json;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet};

const MIGRATION_DAEMON_ID: &str = "server-project-authority-migration-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationMode<'a> {
    DryRun,
    Apply { expected_plan_hash: &'a str },
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProjectAuthorityMigrationReport {
    pub version: u32,
    pub plan_hash: String,
    pub ready: bool,
    pub applied: bool,
    pub legacy_authority_count: usize,
    pub legacy_active_draft_count: usize,
    pub replacement_draft_count: usize,
    pub projects: Vec<ProjectMigrationReport>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProjectMigrationReport {
    pub project_id: String,
    pub project_name: String,
    pub author_user_id: Option<String>,
    pub legacy_authority_count: usize,
    pub legacy_active_draft_count: usize,
    pub replacement_draft_count: usize,
    pub effective_path_content_hash: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MemoryState {
    id: String,
    path: String,
    description: String,
    content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplacementDraft {
    source_id: String,
    target_id: Option<String>,
    path: String,
    description: String,
    content: String,
}

#[derive(Clone, Debug)]
struct AuthorityIdentity {
    resource_id: String,
    revision: i64,
    path: String,
    content_hash: String,
}

#[derive(Clone, Debug)]
struct DraftOverlay {
    draft_id: String,
    author_user_id: String,
    scope: String,
    base_commit_id: Option<String>,
    target_id: Option<String>,
    path: Option<String>,
    status: String,
    version: i64,
    operations: Vec<OverlayOperation>,
}

#[derive(Clone, Debug)]
struct OverlayOperation {
    action: DraftOperationAction,
    target_id: Option<String>,
    path: Option<String>,
    new_path: Option<String>,
    content: Option<DraftResourceContent>,
}

#[derive(Clone, Debug)]
struct ProjectPlan {
    report: ProjectMigrationReport,
    org_id: String,
    author_user_id: Option<String>,
    current_org_commit_id: Option<String>,
    current_project_commit_id: Option<String>,
    authority: Vec<AuthorityIdentity>,
    legacy_drafts: Vec<DraftOverlay>,
    replacements: Vec<ReplacementDraft>,
    before_state_hash: Option<String>,
    blockers: Vec<String>,
}

#[derive(Clone, Debug)]
struct MigrationPlan {
    plan_hash: String,
    projects: Vec<ProjectPlan>,
    blockers: Vec<String>,
}

pub async fn migrate_project_authority(
    pool: &PgPool,
    mode: MigrationMode<'_>,
) -> Result<ProjectAuthorityMigrationReport, ServerError> {
    let mut tx = pool.begin().await?;
    if matches!(mode, MigrationMode::Apply { .. }) {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('project_memory_authority_migration', 0))",
        )
        .execute(&mut *tx)
        .await?;
    }

    let plan = build_plan(&mut tx).await?;
    let mut report = report_for_plan(&plan, false);
    match mode {
        MigrationMode::DryRun => {
            tx.rollback().await?;
            Ok(report)
        }
        MigrationMode::Apply { expected_plan_hash } => {
            if !plan.blockers.is_empty() {
                return Err(ServerError::InvalidRequest(format!(
                    "project authority migration is blocked: {}",
                    plan.blockers.join("; ")
                )));
            }
            if expected_plan_hash != plan.plan_hash {
                return Err(ServerError::InvalidRequest(format!(
                    "project authority migration plan changed: expected {expected_plan_hash}, actual {}",
                    plan.plan_hash
                )));
            }
            apply_plan(&mut tx, &plan).await?;
            tx.commit().await?;
            report.applied = true;
            Ok(report)
        }
    }
}

async fn build_plan(tx: &mut Transaction<'_, Postgres>) -> Result<MigrationPlan, ServerError> {
    let project_rows = sqlx::query(
        "SELECT p.project_id, p.org_id, p.name
         FROM projects p
         WHERE EXISTS (
             SELECT 1 FROM resources r
             WHERE r.project_id = p.project_id
               AND r.scope = 'project'
               AND r.status = 'active'
         ) OR EXISTS (
             SELECT 1 FROM drafts d
             WHERE d.project_id = p.project_id
               AND d.resource_scope = 'project'
               AND d.status IN ('open', 'submitted')
         )
         ORDER BY p.project_id",
    )
    .fetch_all(&mut **tx)
    .await?;

    let mut projects = Vec::with_capacity(project_rows.len());
    let mut blockers = Vec::new();
    for row in project_rows {
        let project_id: String = row.try_get("project_id")?;
        let project_name: String = row.try_get("name")?;
        let org_id: String = row.try_get("org_id")?;
        let project = build_project_plan(tx, project_id, project_name, org_id).await?;
        blockers.extend(project.blockers.iter().cloned());
        projects.push(project);
    }
    let plan_hash = plan_hash(&projects);
    Ok(MigrationPlan {
        plan_hash,
        projects,
        blockers,
    })
}

async fn build_project_plan(
    tx: &mut Transaction<'_, Postgres>,
    project_id: String,
    project_name: String,
    org_id: String,
) -> Result<ProjectPlan, ServerError> {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let member_ids = sqlx::query_scalar::<_, String>(
        "SELECT u.user_id
         FROM project_members pm
         JOIN users u ON u.user_id = pm.user_id
         WHERE pm.project_id = $1 AND u.status = 'active'
         ORDER BY u.user_id",
    )
    .bind(&project_id)
    .fetch_all(&mut **tx)
    .await?;
    let author_user_id = match member_ids.as_slice() {
        [user_id] => Some(user_id.clone()),
        [] => {
            blockers.push(format!(
                "project {project_id} has no active member to own replacement Drafts"
            ));
            None
        }
        _ => {
            blockers.push(format!(
                "project {project_id} has {} active members; replacement Draft ownership is ambiguous",
                member_ids.len()
            ));
            None
        }
    };

    let current_org_commit_id = current_ref(tx, "org", &org_id).await?;
    let current_project_commit_id = current_ref(tx, "project", &project_id).await?;
    let (authority, mut legacy_state) = load_active_project_authority(tx, &project_id).await?;
    if !authority.is_empty() {
        match current_project_commit_id.as_deref() {
            Some(commit_id) => {
                let committed = load_commit_state(tx, commit_id, "project").await?;
                let active_ids = legacy_state.keys().cloned().collect::<BTreeSet<_>>();
                let committed_ids = committed.keys().cloned().collect::<BTreeSet<_>>();
                if active_ids != committed_ids {
                    blockers.push(format!(
                        "project {project_id} current Ref does not contain the same Project Memory identities as active authority"
                    ));
                } else {
                    let mut description_drift = 0;
                    for (resource_id, active) in &legacy_state {
                        let committed = &committed[resource_id];
                        if active.path != committed.path || active.content != committed.content {
                            blockers.push(format!(
                                "project {project_id} current Ref differs from active authority at {resource_id}"
                            ));
                        } else if active.description != committed.description {
                            description_drift += 1;
                        }
                    }
                    if description_drift > 0 {
                        warnings.push(format!(
                            "{description_drift} current Ref descriptions differ from authority; the authority descriptions will be preserved"
                        ));
                    }
                }
            }
            None => blockers.push(format!(
                "project {project_id} has active Project Memory authority but no current Project Commit"
            )),
        }
    }

    let legacy_drafts = load_overlays(tx, &project_id, "project", None).await?;
    if let Some(author_user_id) = author_user_id.as_deref() {
        let foreign_authors = legacy_drafts
            .iter()
            .filter(|draft| draft.author_user_id != author_user_id)
            .map(|draft| draft.author_user_id.clone())
            .collect::<BTreeSet<_>>();
        if !foreign_authors.is_empty() {
            blockers.push(format!(
                "project {project_id} has active Project Drafts owned by another user: {}",
                foreign_authors.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
    }

    let mut commit_cache = BTreeMap::new();
    if let Err(error) =
        apply_overlays(tx, &mut commit_cache, &mut legacy_state, &legacy_drafts).await
    {
        blockers.push(format!(
            "project {project_id} cannot materialize legacy Drafts: {error}"
        ));
        legacy_state.clear();
    }

    let selected_org_authority = load_selected_org_authority(tx, &project_id).await?;
    if let Some(commit_id) = current_project_commit_id.as_deref() {
        let committed = load_commit_state(tx, commit_id, "org").await?;
        let selected_ids = selected_org_authority
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let committed_ids = committed.keys().cloned().collect::<BTreeSet<_>>();
        if selected_ids != committed_ids {
            blockers.push(format!(
                "project {project_id} current Ref does not contain the same selected Organization Memory identities as current authority"
            ));
        } else {
            let mut description_drift = 0;
            for (resource_id, selected) in &selected_org_authority {
                let committed = &committed[resource_id];
                if selected.path != committed.path || selected.content != committed.content {
                    blockers.push(format!(
                        "project {project_id} current Ref differs from selected Organization authority at {resource_id}"
                    ));
                } else if selected.description != committed.description {
                    description_drift += 1;
                }
            }
            if description_drift > 0 {
                warnings.push(format!(
                    "{description_drift} selected Organization Memory descriptions differ from the current Ref; current authority descriptions will be preserved"
                ));
            }
        }
    } else if !selected_org_authority.is_empty() {
        blockers.push(format!(
            "project {project_id} selects Organization Memory but has no current Project Commit"
        ));
    }
    if let Err(error) = validate_materialized_state(&project_id, &selected_org_authority) {
        blockers.push(format!(
            "project {project_id} selected Organization authority is invalid: {error}"
        ));
    }

    let mut full_state = selected_org_authority.clone();
    let org_drafts = if let Some(author_user_id) = author_user_id.as_deref() {
        load_overlays(tx, &project_id, "org", Some(author_user_id)).await?
    } else {
        Vec::new()
    };
    if !org_drafts.is_empty()
        && let Err(error) =
            apply_overlays(tx, &mut commit_cache, &mut full_state, &org_drafts).await
    {
        blockers.push(format!(
            "project {project_id} cannot materialize Organization Drafts: {error}"
        ));
    }
    if let Err(error) = validate_materialized_state(&project_id, &full_state) {
        blockers.push(format!(
            "project {project_id} Organization authority plus active Drafts is invalid: {error}"
        ));
    }

    let mut replacements = plan_replacements(
        &project_id,
        &selected_org_authority,
        &org_drafts,
        &mut full_state,
        legacy_state,
        &mut warnings,
        &mut blockers,
    );
    if let Err(error) = validate_materialized_state(&project_id, &full_state) {
        blockers.push(error.to_string());
    }

    let before_state_hash = blockers.is_empty().then(|| path_content_hash(&full_state));
    replacements.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.source_id.cmp(&right.source_id))
    });
    let report = ProjectMigrationReport {
        project_id: project_id.clone(),
        project_name,
        author_user_id: author_user_id.clone(),
        legacy_authority_count: authority.len(),
        legacy_active_draft_count: legacy_drafts.len(),
        replacement_draft_count: replacements.len(),
        effective_path_content_hash: before_state_hash.clone(),
        warnings,
    };
    Ok(ProjectPlan {
        report,
        org_id,
        author_user_id,
        current_org_commit_id,
        current_project_commit_id,
        authority,
        legacy_drafts,
        replacements,
        before_state_hash,
        blockers,
    })
}

async fn current_ref(
    tx: &mut Transaction<'_, Postgres>,
    scope: &str,
    owner_id: &str,
) -> Result<Option<String>, ServerError> {
    let query = match scope {
        "org" => {
            "SELECT commit_id FROM refs
             WHERE scope = 'org' AND org_id = $1 AND ref_name = 'refs/heads/main'"
        }
        "project" => {
            "SELECT commit_id FROM refs
             WHERE scope = 'project' AND project_id = $1 AND ref_name = 'refs/heads/main'"
        }
        _ => {
            return Err(ServerError::InvalidRequest(format!(
                "unknown migration Ref scope: {scope}"
            )));
        }
    };
    sqlx::query_scalar::<_, Option<String>>(query)
        .bind(owner_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("ref", owner_id))
}

async fn load_active_project_authority(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<(Vec<AuthorityIdentity>, BTreeMap<String, MemoryState>), ServerError> {
    let rows = sqlx::query(
        "SELECT resource_id, revision, path, description, content_hash, body
         FROM resources
         WHERE project_id = $1 AND scope = 'project' AND status = 'active'
         ORDER BY resource_id",
    )
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut identity = Vec::with_capacity(rows.len());
    let mut state = BTreeMap::new();
    for row in rows {
        let resource_id: String = row.try_get("resource_id")?;
        let body: String = row.try_get("body")?;
        let stored_hash: String = row.try_get("content_hash")?;
        let actual_hash = content_hash(&body);
        if stored_hash != actual_hash {
            return Err(ServerError::InvalidRequest(format!(
                "Project Memory {resource_id} stores {stored_hash} but its body hashes to {actual_hash}"
            )));
        }
        let path: String = row.try_get("path")?;
        validate_resource_path(&path)?;
        identity.push(AuthorityIdentity {
            resource_id: resource_id.clone(),
            revision: row.try_get("revision")?,
            path: path.clone(),
            content_hash: stored_hash,
        });
        state.insert(
            resource_id.clone(),
            MemoryState {
                id: resource_id,
                path,
                description: row.try_get("description")?,
                content: body,
            },
        );
    }
    Ok((identity, state))
}

async fn load_selected_org_authority(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<BTreeMap<String, MemoryState>, ServerError> {
    let rows = sqlx::query(
        "SELECT r.resource_id, r.path, r.description, r.body
         FROM project_org_resource_selections selection
         JOIN resources r ON r.resource_id = selection.resource_id
         WHERE selection.project_id = $1
           AND r.scope = 'org'
           AND r.status = 'active'
         ORDER BY r.resource_id",
    )
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let resource_id: String = row.try_get("resource_id")?;
            Ok((
                resource_id.clone(),
                MemoryState {
                    id: resource_id,
                    path: row.try_get("path")?,
                    description: row.try_get("description")?,
                    content: row.try_get("body")?,
                },
            ))
        })
        .collect()
}

async fn load_commit_state(
    tx: &mut Transaction<'_, Postgres>,
    commit_id: &str,
    scope: &str,
) -> Result<BTreeMap<String, MemoryState>, ServerError> {
    let rows = sqlx::query(
        "SELECT entry.item_id, entry.path, entry.description, blob.content
         FROM commits commit
         JOIN tree_entries entry ON entry.tree_id = commit.tree_id
         JOIN blobs blob ON blob.blob_id = entry.blob_id
         WHERE commit.commit_id = $1
           AND entry.scope = $2
           AND entry.resource_kind = 'memory'
         ORDER BY entry.item_id",
    )
    .bind(commit_id)
    .bind(scope)
    .fetch_all(&mut **tx)
    .await?;
    let mut output = BTreeMap::new();
    for row in rows {
        let id: String = row.try_get("item_id")?;
        let path: Option<String> = row.try_get("path")?;
        let path = path.ok_or_else(|| {
            ServerError::InvalidRequest(format!(
                "Commit {commit_id} Memory {id} has no materialization path"
            ))
        })?;
        output.insert(
            id.clone(),
            MemoryState {
                id,
                path,
                description: row.try_get("description")?,
                content: row.try_get("content")?,
            },
        );
    }
    Ok(output)
}

async fn load_overlays(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    scope: &str,
    author_user_id: Option<&str>,
) -> Result<Vec<DraftOverlay>, ServerError> {
    let rows = sqlx::query(
        "SELECT d.draft_id, d.author_user_id, d.base_commit_id, d.target_id,
                d.path AS draft_path, d.status, d.version,
                operation.ordinal, operation.action, operation.target_id AS operation_target_id,
                operation.path AS operation_path, operation.new_path, operation.content
         FROM drafts d
         LEFT JOIN draft_operations operation ON operation.draft_id = d.draft_id
         WHERE d.project_id = $1
           AND d.resource_scope = $2
           AND d.status IN ('open', 'submitted')
           AND ($3::text IS NULL OR d.author_user_id = $3)
         ORDER BY d.created_at, d.draft_id, operation.ordinal",
    )
    .bind(project_id)
    .bind(scope)
    .bind(author_user_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut overlays = Vec::<DraftOverlay>::new();
    for row in rows {
        let draft_id: String = row.try_get("draft_id")?;
        if overlays
            .last()
            .is_none_or(|overlay| overlay.draft_id != draft_id)
        {
            overlays.push(DraftOverlay {
                draft_id: draft_id.clone(),
                author_user_id: row.try_get("author_user_id")?,
                scope: scope.to_owned(),
                base_commit_id: row.try_get("base_commit_id")?,
                target_id: row.try_get("target_id")?,
                path: row.try_get("draft_path")?,
                status: row.try_get("status")?,
                version: row.try_get("version")?,
                operations: Vec::new(),
            });
        }
        let Some(action) = row.try_get::<Option<String>, _>("action")? else {
            continue;
        };
        let action = match action.as_str() {
            "create" => DraftOperationAction::Create,
            "update" => DraftOperationAction::Update,
            "rename" => DraftOperationAction::Rename,
            "delete" => DraftOperationAction::Delete,
            _ => {
                return Err(ServerError::InvalidRequest(format!(
                    "Draft {draft_id} has unknown operation action {action}"
                )));
            }
        };
        overlays
            .last_mut()
            .expect("Draft overlay inserted above")
            .operations
            .push(OverlayOperation {
                action,
                target_id: row.try_get("operation_target_id")?,
                path: row.try_get("operation_path")?,
                new_path: row.try_get("new_path")?,
                content: row
                    .try_get::<Option<Json<DraftResourceContent>>, _>("content")?
                    .map(|content| content.0),
            });
    }
    Ok(overlays)
}

async fn apply_overlays(
    tx: &mut Transaction<'_, Postgres>,
    commit_cache: &mut BTreeMap<(String, String), BTreeMap<String, MemoryState>>,
    state: &mut BTreeMap<String, MemoryState>,
    overlays: &[DraftOverlay],
) -> Result<(), ServerError> {
    for overlay in overlays {
        let commit_scope = overlay.scope.as_str();
        let base = if let Some(commit_id) = overlay.base_commit_id.as_deref() {
            let key = (commit_id.to_owned(), commit_scope.to_owned());
            if !commit_cache.contains_key(&key) {
                let loaded = load_commit_state(tx, commit_id, commit_scope).await?;
                commit_cache.insert(key.clone(), loaded);
            }
            commit_cache.get(&key)
        } else {
            None
        };
        apply_overlay(state, overlay, base)?;
    }
    Ok(())
}

fn apply_overlay(
    resources: &mut BTreeMap<String, MemoryState>,
    overlay: &DraftOverlay,
    base: Option<&BTreeMap<String, MemoryState>>,
) -> Result<(), ServerError> {
    let creates_resource = overlay
        .operations
        .first()
        .is_some_and(|operation| operation.action == DraftOperationAction::Create);
    let base_resource = base.and_then(|base| {
        if let Some(target_id) = overlay.target_id.as_deref() {
            base.get(target_id).cloned()
        } else if !creates_resource {
            overlay.path.as_deref().and_then(|path| {
                base.values()
                    .find(|resource| resource.path == path)
                    .cloned()
            })
        } else {
            None
        }
    });
    let target_key = overlay
        .target_id
        .clone()
        .unwrap_or_else(|| overlay.draft_id.clone());
    let created_resource_id = overlay
        .operations
        .iter()
        .find_map(|operation| operation.target_id.clone())
        .unwrap_or_else(|| overlay.draft_id.clone());
    let mut draft_resources = BTreeMap::new();
    if let Some(base_resource) = base_resource {
        draft_resources.insert(target_key.clone(), base_resource);
    }

    for operation in &overlay.operations {
        match operation.action {
            DraftOperationAction::Create => {
                let path = operation.path.clone().ok_or_else(|| {
                    ServerError::InvalidRequest(format!(
                        "Draft {} create operation has no path",
                        overlay.draft_id
                    ))
                })?;
                validate_resource_path(&path)?;
                let content = operation.content.as_ref().ok_or_else(|| {
                    ServerError::InvalidRequest(format!(
                        "Draft {} create operation has no content",
                        overlay.draft_id
                    ))
                })?;
                draft_resources.clear();
                draft_resources.insert(
                    created_resource_id.clone(),
                    MemoryState {
                        id: created_resource_id.clone(),
                        path,
                        description: content.description.clone().unwrap_or_default(),
                        content: content.content.clone(),
                    },
                );
            }
            DraftOperationAction::Update => {
                let key = operation_target_key(operation, &draft_resources).ok_or_else(|| {
                    ServerError::InvalidRequest(format!(
                        "Draft {} update operation has no resolvable target",
                        overlay.draft_id
                    ))
                })?;
                let existing = draft_resources.remove(&key).ok_or_else(|| {
                    ServerError::InvalidRequest(format!(
                        "Draft {} update target {key} is absent from its Base Commit",
                        overlay.draft_id
                    ))
                })?;
                let content = operation.content.as_ref().ok_or_else(|| {
                    ServerError::InvalidRequest(format!(
                        "Draft {} update operation has no content",
                        overlay.draft_id
                    ))
                })?;
                draft_resources.insert(
                    key,
                    MemoryState {
                        id: existing.id,
                        path: existing.path,
                        description: content.description.clone().unwrap_or(existing.description),
                        content: content.content.clone(),
                    },
                );
            }
            DraftOperationAction::Rename => {
                if let Some(key) = operation_target_key(operation, &draft_resources)
                    && let Some(resource) = draft_resources.get_mut(&key)
                {
                    let new_path = operation.new_path.clone().ok_or_else(|| {
                        ServerError::InvalidRequest(format!(
                            "Draft {} rename operation has no new path",
                            overlay.draft_id
                        ))
                    })?;
                    validate_resource_path(&new_path)?;
                    resource.path = new_path;
                }
            }
            DraftOperationAction::Delete => {
                if let Some(key) = operation_target_key(operation, &draft_resources) {
                    draft_resources.remove(&key);
                }
            }
        }
    }
    if let Some(target_id) = overlay.target_id.as_ref() {
        resources.remove(target_id);
    }
    resources.remove(&overlay.draft_id);
    resources.extend(draft_resources);
    Ok(())
}

fn operation_target_key(
    operation: &OverlayOperation,
    resources: &BTreeMap<String, MemoryState>,
) -> Option<String> {
    operation.target_id.clone().or_else(|| {
        operation.path.as_deref().and_then(|path| {
            resources
                .iter()
                .find(|(_, resource)| resource.path == path)
                .map(|(key, _)| key.clone())
        })
    })
}

fn plan_replacements(
    project_id: &str,
    selected_org_authority: &BTreeMap<String, MemoryState>,
    org_drafts: &[DraftOverlay],
    full_state: &mut BTreeMap<String, MemoryState>,
    legacy_state: BTreeMap<String, MemoryState>,
    warnings: &mut Vec<String>,
    blockers: &mut Vec<String>,
) -> Vec<ReplacementDraft> {
    let mut replacements = Vec::with_capacity(legacy_state.len());
    for legacy in legacy_state.into_values() {
        if let Some(selected) = selected_org_authority
            .values()
            .find(|selected| selected.path == legacy.path)
        {
            let superseding_drafts = org_drafts
                .iter()
                .filter(|draft| draft_targets_resource(draft, selected))
                .map(|draft| draft.draft_id.as_str())
                .collect::<Vec<_>>();
            if !superseding_drafts.is_empty() {
                warnings.push(format!(
                    "legacy Project Memory {} at {} overlaps selected Organization Memory {} and is superseded by active Organization Drafts {}; its history will be discarded without creating a duplicate Draft",
                    legacy.id,
                    legacy.path,
                    selected.id,
                    superseding_drafts.join(", ")
                ));
                continue;
            }

            full_state.insert(
                selected.id.clone(),
                MemoryState {
                    id: selected.id.clone(),
                    path: legacy.path.clone(),
                    description: legacy.description.clone(),
                    content: legacy.content.clone(),
                },
            );
            replacements.push(ReplacementDraft {
                source_id: legacy.id,
                target_id: Some(selected.id.clone()),
                path: legacy.path,
                description: legacy.description,
                content: legacy.content,
            });
            continue;
        }

        if full_state.contains_key(&legacy.id) {
            blockers.push(format!(
                "project {project_id} reuses Memory identity {} across Organization and Project state",
                legacy.id
            ));
            continue;
        }
        full_state.insert(legacy.id.clone(), legacy.clone());
        replacements.push(ReplacementDraft {
            source_id: legacy.id,
            target_id: None,
            path: legacy.path,
            description: legacy.description,
            content: legacy.content,
        });
    }
    replacements
}

fn draft_targets_resource(draft: &DraftOverlay, resource: &MemoryState) -> bool {
    if draft.target_id.as_deref() == Some(resource.id.as_str()) {
        return true;
    }
    let creates_resource = draft
        .operations
        .first()
        .is_some_and(|operation| operation.action == DraftOperationAction::Create);
    if !creates_resource && draft.path.as_deref() == Some(resource.path.as_str()) {
        return true;
    }
    draft.operations.iter().any(|operation| {
        operation.target_id.as_deref() == Some(resource.id.as_str())
            || (!creates_resource && operation.path.as_deref() == Some(resource.path.as_str()))
    })
}

fn validate_materialized_state(
    project_id: &str,
    state: &BTreeMap<String, MemoryState>,
) -> Result<(), ServerError> {
    let mut paths = BTreeMap::new();
    for (key, resource) in state {
        validate_resource_path(&resource.path)?;
        if resource.content.trim().is_empty() {
            return Err(ServerError::InvalidRequest(format!(
                "project {project_id} effective Memory {key} at {} has empty content",
                resource.path
            )));
        }
        insert_materialization_path(
            &mut paths,
            key,
            &materialization_output_path(&resource.path)?,
            &format!("project {project_id} effective memory"),
        )?;
    }
    Ok(())
}

fn path_content_hash(state: &BTreeMap<String, MemoryState>) -> String {
    let mut resources = state.values().collect::<Vec<_>>();
    resources.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut hasher = Sha256::new();
    for resource in resources {
        hasher.update(resource.path.as_bytes());
        hasher.update([0]);
        hasher.update(content_hash(&resource.content).as_bytes());
        hasher.update([0]);
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn plan_hash(projects: &[ProjectPlan]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"project-memory-authority-migration-v1\0");
    for project in projects {
        hash_field(&mut hasher, &project.report.project_id);
        hash_field(&mut hasher, &project.org_id);
        hash_optional(&mut hasher, project.author_user_id.as_deref());
        hash_optional(&mut hasher, project.current_org_commit_id.as_deref());
        hash_optional(&mut hasher, project.current_project_commit_id.as_deref());
        hash_optional(&mut hasher, project.before_state_hash.as_deref());
        for authority in &project.authority {
            hash_field(&mut hasher, &authority.resource_id);
            hash_field(&mut hasher, &authority.revision.to_string());
            hash_field(&mut hasher, &authority.path);
            hash_field(&mut hasher, &authority.content_hash);
        }
        for draft in &project.legacy_drafts {
            hash_field(&mut hasher, &draft.draft_id);
            hash_field(&mut hasher, &draft.version.to_string());
            hash_field(&mut hasher, &draft.status);
            hash_field(&mut hasher, &draft.author_user_id);
        }
        for replacement in &project.replacements {
            hash_field(&mut hasher, &replacement.source_id);
            hash_optional(&mut hasher, replacement.target_id.as_deref());
            hash_field(&mut hasher, &replacement.path);
            hash_field(&mut hasher, &replacement.description);
            hash_field(&mut hasher, &content_hash(&replacement.content));
        }
        for blocker in &project.blockers {
            hash_field(&mut hasher, blocker);
        }
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_optional(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hash_field(hasher, value);
        }
        None => hasher.update([0]),
    }
}

fn report_for_plan(plan: &MigrationPlan, applied: bool) -> ProjectAuthorityMigrationReport {
    ProjectAuthorityMigrationReport {
        version: 1,
        plan_hash: plan.plan_hash.clone(),
        ready: plan.blockers.is_empty(),
        applied,
        legacy_authority_count: plan
            .projects
            .iter()
            .map(|project| project.authority.len())
            .sum(),
        legacy_active_draft_count: plan
            .projects
            .iter()
            .map(|project| project.legacy_drafts.len())
            .sum(),
        replacement_draft_count: plan
            .projects
            .iter()
            .map(|project| project.replacements.len())
            .sum(),
        projects: plan
            .projects
            .iter()
            .map(|project| project.report.clone())
            .collect(),
        blockers: plan.blockers.clone(),
    }
}

async fn apply_plan(
    tx: &mut Transaction<'_, Postgres>,
    plan: &MigrationPlan,
) -> Result<(), ServerError> {
    let org_ids = plan
        .projects
        .iter()
        .map(|project| project.org_id.as_str())
        .collect::<BTreeSet<_>>();
    for org_id in org_ids {
        lock_org_draft_selection_coordination(tx, org_id).await?;
    }

    for project in &plan.projects {
        apply_project_plan(tx, project).await?;
    }
    sqlx::query("ALTER TABLE resources VALIDATE CONSTRAINT resources_no_active_project_authority")
        .execute(&mut **tx)
        .await?;
    sqlx::query("ALTER TABLE drafts VALIDATE CONSTRAINT drafts_no_active_project_authority")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn apply_project_plan(
    tx: &mut Transaction<'_, Postgres>,
    project: &ProjectPlan,
) -> Result<(), ServerError> {
    let author_user_id = project.author_user_id.as_deref().ok_or_else(|| {
        ServerError::InvalidRequest(format!(
            "project {} has no migration Draft owner",
            project.report.project_id
        ))
    })?;
    let current_project_commit_id = current_project_ref(tx, &project.report.project_id).await?;
    if current_project_commit_id != project.current_project_commit_id {
        return Err(ServerError::precondition_failed(
            project.current_project_commit_id.as_deref(),
            current_project_commit_id.as_deref(),
        ));
    }
    let current_org_commit_id = current_ref(tx, "org", &project.org_id).await?;
    if current_org_commit_id != project.current_org_commit_id {
        return Err(ServerError::precondition_failed(
            project.current_org_commit_id.as_deref(),
            current_org_commit_id.as_deref(),
        ));
    }

    let draft_ids = project
        .legacy_drafts
        .iter()
        .map(|draft| draft.draft_id.as_str())
        .collect::<Vec<_>>();
    if !draft_ids.is_empty() {
        sqlx::query(
            "UPDATE reviews review
             SET status = 'rejected', version = version + 1,
                 decision_body = 'Legacy Project authority migrated to Organization Drafts.',
                 approved_result_hash = NULL, decided_by_user_id = $2,
                 decided_at = now(), updated_at = now()
             WHERE review.review_id IN (
                 SELECT review_draft.review_id
                 FROM review_drafts review_draft
                 WHERE review_draft.draft_id = ANY($1)
             ) AND review.status IN ('open', 'approved')",
        )
        .bind(&draft_ids)
        .bind(author_user_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "UPDATE draft_reconciliation_candidates
             SET invalidated_at = now()
             WHERE draft_id = ANY($1) AND invalidated_at IS NULL",
        )
        .bind(&draft_ids)
        .execute(&mut **tx)
        .await?;
        for draft in &project.legacy_drafts {
            let next_version: i64 = sqlx::query_scalar(
                "UPDATE drafts
                 SET status = 'discarded', version = version + 1, updated_at = now()
                 WHERE draft_id = $1
                   AND version = $2
                   AND status IN ('open', 'submitted')
                 RETURNING version",
            )
            .bind(&draft.draft_id)
            .bind(draft.version)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| {
                ServerError::InvalidRequest(format!(
                    "legacy Draft {} changed after planning",
                    draft.draft_id
                ))
            })?;
            insert_draft_event(
                tx,
                &draft.draft_id,
                &project.report.project_id,
                DraftEventType::Discarded,
                next_version,
                None,
            )
            .await?;
        }
    }

    let resource_ids = project
        .authority
        .iter()
        .map(|resource| resource.resource_id.as_str())
        .collect::<Vec<_>>();
    if !resource_ids.is_empty() {
        let archived = sqlx::query(
            "UPDATE resources
             SET status = 'archived', revision = revision + 1, updated_at = now()
             WHERE resource_id = ANY($1)
               AND scope = 'project'
               AND status = 'active'",
        )
        .bind(&resource_ids)
        .execute(&mut **tx)
        .await?;
        if archived.rows_affected() != resource_ids.len() as u64 {
            return Err(ServerError::InvalidRequest(format!(
                "project {} authority changed after planning",
                project.report.project_id
            )));
        }
    }

    for replacement in &project.replacements {
        let action = if replacement.target_id.is_some() {
            DraftOperationAction::Update
        } else {
            DraftOperationAction::Create
        };
        let resource = DraftResourceRef {
            scope: ResourceScope::Org,
            id: replacement.target_id.clone(),
            path: replacement
                .target_id
                .is_none()
                .then(|| replacement.path.clone()),
        };
        create_draft(
            tx,
            author_user_id,
            CreateDraftRequest {
                daemon_installation_id: MIGRATION_DAEMON_ID.to_owned(),
                project_id: project.report.project_id.clone(),
                base_commit_id: project.current_org_commit_id.clone(),
                title: format!("Migrate Project Memory: {}", replacement.path),
                description: Some(
                    "Converted from legacy Project Memory state; publication remains subject to Organization Review."
                        .to_owned(),
                ),
                resource: resource.clone(),
                operations: vec![DraftOperationInput {
                    action,
                    resource,
                    content: Some(DraftResourceContent {
                        description: Some(replacement.description.clone()),
                        content: replacement.content.clone(),
                    }),
                    new_path: None,
                }],
            },
        )
        .await?;
    }

    verify_converted_effective_state(tx, project, author_user_id).await?;

    let commit_id = create_project_commit(
        tx,
        &project.report.project_id,
        project.current_project_commit_id.as_deref(),
    )
    .await?;
    advance_project_ref(tx, &project.report.project_id, &commit_id).await?;
    sqlx::query(
        "INSERT INTO audit_events (
            event_id, org_id, actor_user_id, action, target_type, target_id
         ) VALUES ($1, $2, $3, 'project_memory_authority_migrated', 'project', $4)",
    )
    .bind(prefixed_id("aud"))
    .bind(&project.org_id)
    .bind(author_user_id)
    .bind(&project.report.project_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn verify_converted_effective_state(
    tx: &mut Transaction<'_, Postgres>,
    project: &ProjectPlan,
    author_user_id: &str,
) -> Result<(), ServerError> {
    let mut state = load_selected_org_authority(tx, &project.report.project_id).await?;
    let drafts = load_overlays(tx, &project.report.project_id, "org", Some(author_user_id)).await?;
    apply_overlays(tx, &mut BTreeMap::new(), &mut state, &drafts).await?;
    validate_materialized_state(&project.report.project_id, &state)?;
    let actual_hash = path_content_hash(&state);
    let expected_hash = project.before_state_hash.as_deref().ok_or_else(|| {
        ServerError::InvalidRequest(format!(
            "project {} has no planned Effective Memory hash",
            project.report.project_id
        ))
    })?;
    if actual_hash != expected_hash {
        return Err(ServerError::InvalidRequest(format!(
            "project {} Effective Memory changed during conversion: expected {expected_hash}, actual {actual_hash}",
            project.report.project_id
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(id: &str, path: &str, content: &str) -> MemoryState {
        MemoryState {
            id: id.to_owned(),
            path: path.to_owned(),
            description: format!("Description for {id}"),
            content: content.to_owned(),
        }
    }

    #[test]
    fn exact_selected_org_path_becomes_an_update_replacement() {
        let selected = memory("mem_org", "workflow/CODING.md", "Org authority");
        let legacy = memory("draft_project", "workflow/CODING.md", "Project proposal");
        let selected_state = BTreeMap::from([(selected.id.clone(), selected.clone())]);
        let mut full_state = selected_state.clone();
        let mut warnings = Vec::new();
        let mut blockers = Vec::new();

        let replacements = plan_replacements(
            "prj_test",
            &selected_state,
            &[],
            &mut full_state,
            BTreeMap::from([(legacy.id.clone(), legacy)]),
            &mut warnings,
            &mut blockers,
        );

        assert!(warnings.is_empty());
        assert!(blockers.is_empty());
        assert_eq!(replacements.len(), 1);
        assert_eq!(replacements[0].target_id.as_deref(), Some("mem_org"));
        assert_eq!(full_state.len(), 1);
        assert_eq!(full_state["mem_org"].content, "Project proposal");
    }

    #[test]
    fn active_org_draft_supersedes_an_exact_legacy_project_path() {
        let selected = memory("mem_org", "workflow/CODING.md", "Org authority");
        let legacy = memory(
            "draft_project",
            "workflow/CODING.md",
            "Old Project proposal",
        );
        let selected_state = BTreeMap::from([(selected.id.clone(), selected)]);
        let mut full_state = BTreeMap::from([(
            "mem_org".to_owned(),
            memory("mem_org", "workflow/CODING.md", "Current Org proposal"),
        )]);
        let org_drafts = vec![DraftOverlay {
            draft_id: "drf_org".to_owned(),
            author_user_id: "usr_test".to_owned(),
            scope: "org".to_owned(),
            base_commit_id: None,
            target_id: Some("mem_org".to_owned()),
            path: None,
            status: "open".to_owned(),
            version: 1,
            operations: Vec::new(),
        }];
        let mut warnings = Vec::new();
        let mut blockers = Vec::new();

        let replacements = plan_replacements(
            "prj_test",
            &selected_state,
            &org_drafts,
            &mut full_state,
            BTreeMap::from([(legacy.id.clone(), legacy)]),
            &mut warnings,
            &mut blockers,
        );

        assert!(replacements.is_empty());
        assert!(blockers.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("superseded by active Organization Drafts drf_org"));
        assert_eq!(full_state["mem_org"].content, "Current Org proposal");
    }
}
