//! SQL queries and persistence for draft resources.

use super::model::{
    apply_operations_to_state, content_for_kind, diff_resource_states, draft_event_type,
    draft_operation_action, draft_status, ensure_writable_draft_scope, merge_resource_states,
    state_hash, validate_draft_operation_resource, validate_draft_resource,
    validate_new_resource_draft_operations,
};
use super::service::{
    canonicalize_org_draft_target_is_selected, canonicalize_org_draft_targets_are_selected,
    target_ref_for_draft,
};
use crate::app::auth::AuthPrincipal;
use crate::app::commit::service::{
    load_org_ref, load_project_ref, validate_org_commit, validate_project_commit,
};
use crate::app::draft::dto::{
    CreateDraftRebaseRequest, CreateDraftRequest, Draft, DraftCoordination, DraftDetail,
    DraftEvent, DraftEventListResponse, DraftEventType, DraftFreshness, DraftListResponse,
    DraftOperation, DraftOperationAction, DraftOperationBatchRequest, DraftOperationBatchResponse,
    DraftOperationInput, DraftRebaseResult, DraftReconciliationCandidate,
    DraftReconciliationStatus, DraftResourceContent, DraftResourceRef, DraftSyncState,
    DraftSyncStatus, ReconciliationCandidateStatus, ReconciliationConflict,
    ReconciliationConflictKind, ReconciliationResourceState, UpdateDraftRequest,
};
use crate::app::memory::dto::ResourceScope;
use crate::app::memory::model::resource_scope;
use crate::app::memory::service::{
    lock_org_draft_selection_coordination, lock_org_draft_selection_coordination_for_project,
    project_org_id,
};
use crate::app::organization::dto::UserRef;
use crate::app::review::service::{
    load_review, load_review_draft_ids, refresh_review_after_draft_content_change,
    review_result_hash,
};
use crate::dto::DeleteResult;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use crate::pagination::page_info;
use sqlx::types::Json;
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;

pub(crate) async fn user_ref(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
) -> Result<UserRef, ServerError> {
    let row = sqlx::query(
        "SELECT user_id, email, display_name, avatar_url, role
         FROM users
         WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("user", user_id))?;
    user_ref_from_row(&row)
}

pub(crate) fn user_ref_from_row(row: &sqlx::postgres::PgRow) -> Result<UserRef, ServerError> {
    Ok(UserRef {
        user_id: row.try_get("user_id")?,
        email: row.try_get("email")?,
        display_name: row.try_get("display_name")?,
        avatar_url: row.try_get("avatar_url")?,
        role: row.try_get("role")?,
    })
}

pub(crate) async fn resolve_org_draft_target_id(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    base_commit_id: Option<&str>,
    resource: &DraftResourceRef,
    allow_historical_lookup: bool,
) -> Result<Option<String>, ServerError> {
    if let Some(resource_id) = resource.id.as_ref() {
        return Ok(Some(resource_id.clone()));
    }
    let Some(path) = resource.path.as_deref() else {
        return Ok(None);
    };

    if let Some(base_commit_id) = base_commit_id {
        let resource_id = sqlx::query_scalar::<_, String>(
            "SELECT entry.item_id
             FROM commits AS commit
             JOIN tree_entries AS entry ON entry.tree_id = commit.tree_id
             WHERE commit.commit_id = $1
               AND commit.org_id = $2
               AND commit.scope = 'org'
               AND entry.scope = 'org'
               AND entry.resource_kind = 'memory'
               AND entry.path = $3",
        )
        .bind(base_commit_id)
        .bind(org_id)
        .bind(path)
        .fetch_optional(&mut **tx)
        .await?;
        if resource_id.is_some() {
            return Ok(resource_id);
        }
    }

    let current_resource_id = sqlx::query_scalar::<_, String>(
        "SELECT resource_id
         FROM resources
         WHERE org_id = $1
           AND scope = 'org'
           AND status = 'active'
           AND path = $2",
    )
    .bind(org_id)
    .bind(path)
    .fetch_optional(&mut **tx)
    .await?;
    if current_resource_id.is_some() || !allow_historical_lookup {
        return Ok(current_resource_id);
    }

    let resource_ids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT entry.item_id
         FROM commits AS commit
         JOIN tree_entries AS entry ON entry.tree_id = commit.tree_id
         WHERE commit.org_id = $1
           AND commit.scope = 'org'
           AND entry.scope = 'org'
           AND entry.resource_kind = 'memory'
           AND entry.path = $2
         ORDER BY entry.item_id
         LIMIT 2",
    )
    .bind(org_id)
    .bind(path)
    .fetch_all(&mut **tx)
    .await?;
    match resource_ids.as_slice() {
        [] => Ok(None),
        [resource_id] => Ok(Some(resource_id.clone())),
        _ => Err(ServerError::InvalidRequest(format!(
            "Organization Memory path {path} has referred to multiple resources; target it by resource id"
        ))),
    }
}

pub(crate) async fn validate_org_draft_target_is_selected(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
    resource: &DraftResourceRef,
) -> Result<(), ServerError> {
    let selected = if let Some(resource_id) = resource.id.as_deref() {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1
                FROM project_org_resource_selections s
                JOIN resources r ON r.resource_id = s.resource_id
                WHERE s.project_id = $1
                  AND r.resource_id = $2
                  AND r.org_id = $3
                  AND r.scope = 'org'
                  AND r.status = 'active'
             )",
        )
        .bind(project_id)
        .bind(resource_id)
        .bind(org_id)
        .fetch_one(&mut **tx)
        .await?
    } else if let Some(path) = resource.path.as_deref() {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1
                FROM project_org_resource_selections s
                JOIN resources r ON r.resource_id = s.resource_id
                WHERE s.project_id = $1
                  AND r.org_id = $2
                  AND r.scope = 'org'
                  AND r.path = $3
                  AND r.status = 'active'
             )",
        )
        .bind(project_id)
        .bind(org_id)
        .bind(path)
        .fetch_one(&mut **tx)
        .await?
    } else {
        false
    };
    if selected {
        return Ok(());
    }
    Err(ServerError::InvalidRequest(
        "an Organization Memory Draft may target only Memory currently selected by its carrying Project"
            .to_owned(),
    ))
}

pub(crate) async fn insert_draft_operation(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    input: DraftOperationInput,
) -> Result<String, ServerError> {
    let operation_id = prefixed_id("dop");
    // Serialize every allocator, including future call sites that do not
    // already hold the draft row lock. A per-draft MAX is safe once this lock
    // is held and keeps replacement operation sets densely ordered.
    sqlx::query("SELECT draft_id FROM drafts WHERE draft_id = $1 FOR UPDATE")
        .bind(draft_id)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO draft_operations (
            operation_id, draft_id, action, resource_scope, resource_kind, target_id, path,
            new_path, content, ordinal
         )
         SELECT $1, $2, $3, $4, 'memory', $5, $6, $7, $8,
                COALESCE(MAX(ordinal), 0) + 1
         FROM draft_operations
         WHERE draft_id = $2",
    )
    .bind(&operation_id)
    .bind(draft_id)
    .bind(input.action.as_str())
    .bind(input.resource.scope.as_str())
    .bind(&input.resource.id)
    .bind(&input.resource.path)
    .bind(&input.new_path)
    .bind(input.content.as_ref().map(Json))
    .execute(&mut **tx)
    .await?;
    Ok(operation_id)
}

pub(crate) async fn append_draft_operation_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    expected_draft_version: i64,
    mut operation: DraftOperationInput,
    event_daemon_installation_id: Option<&str>,
    org_coordination_already_locked: bool,
) -> Result<i64, ServerError> {
    let identity = sqlx::query(
        "SELECT project_id, resource_scope
         FROM drafts
         WHERE draft_id = $1",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let identity_scope = resource_scope(identity.try_get::<String, _>("resource_scope")?.as_str())?;
    ensure_writable_draft_scope(identity_scope)?;
    if identity_scope == ResourceScope::Org && !org_coordination_already_locked {
        lock_org_draft_selection_coordination_for_project(
            tx,
            &identity.try_get::<String, _>("project_id")?,
        )
        .await?;
    }
    let row = sqlx::query(
        "SELECT d.status, d.version, d.project_id, d.resource_scope, d.resource_kind,
                d.base_commit_id,
                COALESCE((
                    SELECT operation.action = 'create'
                    FROM draft_operations AS operation
                    WHERE operation.draft_id = d.draft_id
                    ORDER BY operation.ordinal
                    LIMIT 1
                ), FALSE) AS creates_resource
         FROM drafts AS d
         WHERE d.draft_id = $1
         FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let status: String = row.try_get("status")?;
    let version: i64 = row.try_get("version")?;
    let scope = resource_scope(row.try_get::<String, _>("resource_scope")?.as_str())?;
    let draft_resource = DraftResourceRef {
        scope,
        id: None,
        path: None,
    };

    if status != "open" && status != "submitted" {
        return Err(ServerError::invalid_transition("draft", &status, "append"));
    }
    if version != expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            expected_draft_version,
            version,
        ));
    }
    let creates_resource: bool = row.try_get("creates_resource")?;
    if creates_resource && operation.action == DraftOperationAction::Delete {
        return Err(ServerError::InvalidRequest(
            "a draft-created resource must be discarded instead of deleted".to_owned(),
        ));
    }
    validate_draft_operation_resource(&draft_resource, &operation)?;
    if scope == ResourceScope::Org
        && !creates_resource
        && operation.action != DraftOperationAction::Create
    {
        let project_id: String = row.try_get("project_id")?;
        let org_id = project_org_id(tx, &project_id).await?;
        let base_commit_id: Option<String> = row.try_get("base_commit_id")?;
        canonicalize_org_draft_target_is_selected(
            tx,
            &project_id,
            &org_id,
            base_commit_id.as_deref(),
            &mut operation.resource,
        )
        .await?;
    }

    insert_draft_operation(tx, draft_id, operation).await?;
    let updated = sqlx::query(
        "UPDATE drafts
         SET version = version + 1, updated_at = now()
         WHERE draft_id = $1
         RETURNING project_id, version",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?;
    invalidate_draft_candidates(tx, draft_id).await?;
    refresh_review_after_draft_content_change(tx, draft_id).await?;
    insert_draft_event(
        tx,
        draft_id,
        &updated.try_get::<String, _>("project_id")?,
        DraftEventType::OperationAppended,
        updated.try_get("version")?,
        event_daemon_installation_id,
    )
    .await
}

pub(crate) async fn invalidate_draft_candidates(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE draft_reconciliation_candidates
         SET invalidated_at = now()
         WHERE draft_id = $1 AND invalidated_at IS NULL",
    )
    .bind(draft_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn insert_draft_event(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    project_id: &str,
    event_type: DraftEventType,
    version: i64,
    daemon_installation_id: Option<&str>,
) -> Result<i64, ServerError> {
    let row = sqlx::query(
        "INSERT INTO draft_events (
            event_id, draft_id, project_id, event_type, version, daemon_installation_id
         )
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING server_sequence",
    )
    .bind(prefixed_id("evt"))
    .bind(draft_id)
    .bind(project_id)
    .bind(event_type.as_str())
    .bind(version)
    .bind(daemon_installation_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.try_get("server_sequence")?)
}

pub(crate) async fn load_draft_detail(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<DraftDetail, ServerError> {
    let row = sqlx::query(
        "SELECT
            d.draft_id, d.project_id, d.base_commit_id, d.title, d.description,
            d.status, d.version,
            d.resource_scope, d.resource_kind, d.target_id, d.path, d.daemon_installation_id,
            d.created_at, d.updated_at,
            u.user_id, u.email, u.display_name, u.avatar_url, u.role
         FROM drafts d
         JOIN users u ON u.user_id = d.author_user_id
         WHERE d.draft_id = $1",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("draft", draft_id))?;

    let daemon_installation_id: String = row.try_get("daemon_installation_id")?;
    let resource_scope = resource_scope(row.try_get::<String, _>("resource_scope")?.as_str())?;
    let resource = DraftResourceRef {
        scope: resource_scope,
        id: row.try_get("target_id")?,
        path: row.try_get("path")?,
    };
    let operations = load_draft_operations(tx, draft_id).await?;
    let allow_path_lookup = operations
        .first()
        .is_none_or(|operation| operation.input.action != DraftOperationAction::Create);
    let coordination = load_draft_coordination(
        tx,
        draft_id,
        row.try_get("project_id")?,
        row.try_get("base_commit_id")?,
        row.try_get("version")?,
        &resource,
        allow_path_lookup,
    )
    .await?;
    let draft = Draft {
        draft_id: row.try_get("draft_id")?,
        project_id: row.try_get("project_id")?,
        base_commit_id: row.try_get("base_commit_id")?,
        author: user_ref_from_row(&row)?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        resource,
        status: draft_status(row.try_get::<String, _>("status")?.as_str())?,
        coordination,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    };
    Ok(DraftDetail {
        draft,
        operations,
        sync_state: DraftSyncState {
            status: DraftSyncStatus::Synced,
            server_cursor: Some(format!(
                "draft:{}:{}",
                draft_id,
                row.try_get::<i64, _>("version")?
            )),
            daemon_installation_id: Some(daemon_installation_id),
        },
    })
}

pub(crate) async fn load_draft_coordination(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    project_id: String,
    base_commit_id: Option<String>,
    draft_version: i64,
    resource: &DraftResourceRef,
    allow_path_lookup: bool,
) -> Result<DraftCoordination, ServerError> {
    let current_commit_id = match resource.scope {
        ResourceScope::Org => {
            let org_id = project_org_id(tx, &project_id).await?;
            load_org_ref(tx, &org_id).await?.commit_id
        }
        ResourceScope::Project => load_project_ref(tx, &project_id).await?.commit_id,
    };
    let freshness = if base_commit_id == current_commit_id {
        DraftFreshness::Current
    } else {
        DraftFreshness::Behind
    };
    let has_upstream_resource_changes = if freshness == DraftFreshness::Behind {
        let base_state =
            resource_state_at_commit(tx, base_commit_id.as_deref(), resource, allow_path_lookup)
                .await?;
        let current_state = resource_state_at_commit(
            tx,
            current_commit_id.as_deref(),
            resource,
            allow_path_lookup,
        )
        .await?;
        base_state != current_state
    } else {
        false
    };
    let candidate = if freshness == DraftFreshness::Behind {
        sqlx::query(
            "SELECT candidate_id, status
             FROM draft_reconciliation_candidates
             WHERE draft_id = $1 AND draft_version = $2
               AND base_commit_id IS NOT DISTINCT FROM $3
               AND current_commit_id IS NOT DISTINCT FROM $4
               AND invalidated_at IS NULL
             ORDER BY created_at DESC
             LIMIT 1",
        )
        .bind(draft_id)
        .bind(draft_version)
        .bind(&base_commit_id)
        .bind(&current_commit_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        None
    };
    let (reconciliation, candidate_id) = match candidate {
        Some(row) => {
            let status: String = row.try_get("status")?;
            (
                match status.as_str() {
                    "clean" => DraftReconciliationStatus::Clean,
                    "conflicts" => DraftReconciliationStatus::Conflicts,
                    _ => {
                        return Err(ServerError::InvalidRequest(format!(
                            "unknown reconciliation status: {status}"
                        )));
                    }
                },
                Some(row.try_get("candidate_id")?),
            )
        }
        None => (DraftReconciliationStatus::Unknown, None),
    };
    Ok(DraftCoordination {
        freshness,
        current_commit_id,
        has_upstream_resource_changes,
        reconciliation,
        candidate_id,
    })
}

pub(crate) async fn load_draft_operations(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Vec<DraftOperation>, ServerError> {
    let rows = sqlx::query(
        "SELECT operation_id, action, resource_scope, resource_kind, target_id, path,
                new_path, content, created_at
         FROM draft_operations
         WHERE draft_id = $1
         ORDER BY ordinal",
    )
    .bind(draft_id)
    .fetch_all(&mut **tx)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(DraftOperation {
                input: DraftOperationInput {
                    action: draft_operation_action(row.try_get::<String, _>("action")?.as_str())?,
                    resource: DraftResourceRef {
                        scope: resource_scope(
                            row.try_get::<String, _>("resource_scope")?.as_str(),
                        )?,
                        id: row.try_get("target_id")?,
                        path: row.try_get("path")?,
                    },
                    content: row
                        .try_get::<Option<Json<DraftResourceContent>>, _>("content")?
                        .map(|value| value.0),
                    new_path: row.try_get("new_path")?,
                },
                operation_id: row.try_get("operation_id")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

pub(crate) async fn resource_state_at_commit(
    tx: &mut Transaction<'_, Postgres>,
    commit_id: Option<&str>,
    resource: &DraftResourceRef,
    allow_path_lookup: bool,
) -> Result<ReconciliationResourceState, ServerError> {
    let Some(commit_id) = commit_id else {
        return Ok(ReconciliationResourceState {
            exists: false,
            resource: resource.clone(),
            content: None,
        });
    };
    let row = sqlx::query(
        "SELECT e.item_id, e.path, e.blob_id, b.content
         FROM commits c
         LEFT JOIN LATERAL (
             SELECT item_id, path, blob_id
             FROM tree_entries
             WHERE tree_id = c.tree_id
               AND resource_kind = 'memory'
               AND scope = $2
               AND (
                   ($3::TEXT IS NOT NULL AND item_id = $3)
                   OR ($3::TEXT IS NULL AND $4 AND path = $5)
               )
             ORDER BY item_id
             LIMIT 1
         ) e ON TRUE
         LEFT JOIN blobs b ON b.blob_id = e.blob_id
         WHERE c.commit_id = $1",
    )
    .bind(commit_id)
    .bind(resource.scope.as_str())
    .bind(resource.id.as_deref())
    .bind(allow_path_lookup)
    .bind(resource.path.as_deref())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("commit", commit_id))?;

    let Some(id) = row.try_get::<Option<String>, _>("item_id")? else {
        return Ok(ReconciliationResourceState {
            exists: false,
            resource: resource.clone(),
            content: None,
        });
    };
    let blob_id: String = row.try_get("blob_id")?;
    let content = row
        .try_get::<Option<String>, _>("content")?
        .ok_or_else(|| {
            ServerError::InvalidRequest(format!("commit {commit_id} is missing blob {blob_id}"))
        })?;
    Ok(ReconciliationResourceState {
        exists: true,
        resource: DraftResourceRef {
            scope: resource.scope,
            id: Some(id),
            path: row.try_get("path")?,
        },
        content: Some(content_for_kind("memory", content, None)),
    })
}

pub(crate) async fn path_is_occupied(
    tx: &mut Transaction<'_, Postgres>,
    commit_id: Option<&str>,
    state: &ReconciliationResourceState,
) -> Result<bool, ServerError> {
    if !state.exists {
        return Ok(false);
    }
    let Some(path) = state.resource.path.as_deref() else {
        return Ok(false);
    };
    let Some(commit_id) = commit_id else {
        return Ok(false);
    };
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
             SELECT 1
             FROM tree_entries e
             WHERE e.tree_id = c.tree_id
               AND e.resource_kind = 'memory'
               AND e.scope = $2
               AND e.path = $3
               AND e.item_id IS DISTINCT FROM $4
         )
         FROM commits c
         WHERE c.commit_id = $1",
    )
    .bind(commit_id)
    .bind(state.resource.scope.as_str())
    .bind(path)
    .bind(state.resource.id.as_deref())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("commit", commit_id))
}

pub(crate) async fn draft_result_state(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<ReconciliationResourceState, ServerError> {
    let row = sqlx::query(
        "SELECT base_commit_id, resource_scope, resource_kind, target_id, path
         FROM drafts WHERE draft_id = $1",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let operations = load_draft_operations(tx, draft_id).await?;
    let resource = DraftResourceRef {
        scope: resource_scope(row.try_get::<String, _>("resource_scope")?.as_str())?,
        id: row.try_get("target_id")?,
        path: row.try_get("path")?,
    };
    let allow_path_lookup = operations
        .first()
        .is_none_or(|operation| operation.input.action != DraftOperationAction::Create);
    let base_commit_id: Option<String> = row.try_get("base_commit_id")?;
    let base =
        resource_state_at_commit(tx, base_commit_id.as_deref(), &resource, allow_path_lookup)
            .await?;
    apply_operations_to_state(base, &operations)
}

pub(crate) async fn create_reconciliation_candidate_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    expected_draft_version: i64,
) -> Result<DraftReconciliationCandidate, ServerError> {
    let row = sqlx::query(
        "SELECT project_id, base_commit_id, resource_scope, resource_kind,
                target_id, path, status, version
         FROM drafts
         WHERE draft_id = $1
         FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let draft_version: i64 = row.try_get("version")?;
    if draft_version != expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            expected_draft_version,
            draft_version,
        ));
    }
    let lifecycle: String = row.try_get("status")?;
    if lifecycle != "open" && lifecycle != "submitted" {
        return Err(ServerError::invalid_transition(
            "draft",
            &lifecycle,
            "reconciled",
        ));
    }
    let project_id: String = row.try_get("project_id")?;
    let scope = resource_scope(row.try_get::<String, _>("resource_scope")?.as_str())?;
    let base_commit_id: Option<String> = row.try_get("base_commit_id")?;
    let current_commit_id = target_ref_for_draft(tx, &project_id, scope).await?;
    if base_commit_id == current_commit_id {
        return Err(ServerError::DraftAlreadyCurrent {
            draft_id: draft_id.to_owned(),
        });
    }

    if let Some(existing_id) = sqlx::query_scalar::<_, String>(
        "SELECT candidate_id
         FROM draft_reconciliation_candidates
         WHERE draft_id = $1 AND draft_version = $2
           AND base_commit_id IS NOT DISTINCT FROM $3
           AND current_commit_id IS NOT DISTINCT FROM $4
           AND invalidated_at IS NULL
         LIMIT 1",
    )
    .bind(draft_id)
    .bind(draft_version)
    .bind(&base_commit_id)
    .bind(&current_commit_id)
    .fetch_optional(&mut **tx)
    .await?
    {
        return load_reconciliation_candidate(tx, draft_id, &existing_id).await;
    }

    invalidate_draft_candidates(tx, draft_id).await?;
    let mut resource = DraftResourceRef {
        scope,
        id: row.try_get("target_id")?,
        path: row.try_get("path")?,
    };
    let operations = load_draft_operations(tx, draft_id).await?;
    let allow_path_lookup = operations
        .first()
        .is_none_or(|operation| operation.input.action != DraftOperationAction::Create);
    if scope == ResourceScope::Org && resource.id.is_none() && allow_path_lookup {
        let org_id = project_org_id(tx, &project_id).await?;
        resource.id =
            resolve_org_draft_target_id(tx, &org_id, base_commit_id.as_deref(), &resource, true)
                .await?;
    }
    let base_state =
        resource_state_at_commit(tx, base_commit_id.as_deref(), &resource, allow_path_lookup)
            .await?;
    let current_state = resource_state_at_commit(
        tx,
        current_commit_id.as_deref(),
        &resource,
        allow_path_lookup,
    )
    .await?;
    let draft_state = apply_operations_to_state(base_state.clone(), &operations)?;
    let (mut proposed_state, mut conflicts) =
        merge_resource_states(&base_state, &current_state, &draft_state);
    if let Some(proposed) = proposed_state.as_ref()
        && path_is_occupied(tx, current_commit_id.as_deref(), proposed).await?
    {
        conflicts.push(ReconciliationConflict {
            kind: ReconciliationConflictKind::PathOccupied,
            field: "path".to_owned(),
            base: base_state.resource.path.clone(),
            current: current_state.resource.path.clone(),
            draft: proposed.resource.path.clone(),
        });
        proposed_state = None;
    }
    let status = if conflicts.is_empty() {
        ReconciliationCandidateStatus::Clean
    } else {
        ReconciliationCandidateStatus::Conflicts
    };
    let result_hash = proposed_state.as_ref().map(state_hash).transpose()?;
    let candidate_id = prefixed_id("rcn");
    sqlx::query(
        "INSERT INTO draft_reconciliation_candidates (
            candidate_id, draft_id, draft_version, base_commit_id, current_commit_id,
            status, base_state, current_state, draft_state, proposed_state,
            conflicts, result_hash
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(&candidate_id)
    .bind(draft_id)
    .bind(draft_version)
    .bind(&base_commit_id)
    .bind(&current_commit_id)
    .bind(match status {
        ReconciliationCandidateStatus::Clean => "clean",
        ReconciliationCandidateStatus::Conflicts => "conflicts",
    })
    .bind(Json(&base_state))
    .bind(Json(&current_state))
    .bind(Json(&draft_state))
    .bind(proposed_state.as_ref().map(Json))
    .bind(Json(&conflicts))
    .bind(&result_hash)
    .execute(&mut **tx)
    .await?;
    load_reconciliation_candidate(tx, draft_id, &candidate_id).await
}

pub(crate) async fn load_reconciliation_candidate(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    candidate_id: &str,
) -> Result<DraftReconciliationCandidate, ServerError> {
    let row = sqlx::query(
        "SELECT candidate_id, draft_id, draft_version, base_commit_id, current_commit_id,
                status, base_state, current_state, draft_state, proposed_state,
                conflicts, result_hash, created_at, invalidated_at
         FROM draft_reconciliation_candidates
         WHERE candidate_id = $1 AND draft_id = $2",
    )
    .bind(candidate_id)
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("reconciliation_candidate", candidate_id))?;

    let draft_row = sqlx::query(
        "SELECT project_id, base_commit_id, resource_scope, version
         FROM drafts WHERE draft_id = $1",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?;
    let scope = resource_scope(draft_row.try_get::<String, _>("resource_scope")?.as_str())?;
    let current =
        target_ref_for_draft(tx, &draft_row.try_get::<String, _>("project_id")?, scope).await?;
    let invalidated_at: Option<OffsetDateTime> = row.try_get("invalidated_at")?;
    let valid = invalidated_at.is_none()
        && row.try_get::<i64, _>("draft_version")? == draft_row.try_get::<i64, _>("version")?
        && row.try_get::<Option<String>, _>("base_commit_id")?
            == draft_row.try_get::<Option<String>, _>("base_commit_id")?
        && row.try_get::<Option<String>, _>("current_commit_id")? == current;
    if !valid && invalidated_at.is_none() {
        sqlx::query(
            "UPDATE draft_reconciliation_candidates SET invalidated_at = now()
             WHERE candidate_id = $1 AND invalidated_at IS NULL",
        )
        .bind(candidate_id)
        .execute(&mut **tx)
        .await?;
    }
    let status: String = row.try_get("status")?;
    Ok(DraftReconciliationCandidate {
        candidate_id: row.try_get("candidate_id")?,
        draft_id: row.try_get("draft_id")?,
        draft_version: row.try_get("draft_version")?,
        base_commit_id: row.try_get("base_commit_id")?,
        current_commit_id: row.try_get("current_commit_id")?,
        status: match status.as_str() {
            "clean" => ReconciliationCandidateStatus::Clean,
            "conflicts" => ReconciliationCandidateStatus::Conflicts,
            _ => {
                return Err(ServerError::InvalidRequest(format!(
                    "unknown candidate status: {status}"
                )));
            }
        },
        base_state: row
            .try_get::<Json<ReconciliationResourceState>, _>("base_state")?
            .0,
        current_state: row
            .try_get::<Json<ReconciliationResourceState>, _>("current_state")?
            .0,
        draft_state: row
            .try_get::<Json<ReconciliationResourceState>, _>("draft_state")?
            .0,
        proposed_state: row
            .try_get::<Option<Json<ReconciliationResourceState>>, _>("proposed_state")?
            .map(|state| state.0),
        conflicts: row
            .try_get::<Json<Vec<ReconciliationConflict>>, _>("conflicts")?
            .0,
        result_hash: row.try_get("result_hash")?,
        valid,
        created_at: row.try_get("created_at")?,
        invalidated_at: if valid {
            None
        } else {
            invalidated_at.or_else(|| Some(OffsetDateTime::now_utc()))
        },
    })
}

pub(crate) async fn apply_draft_rebase_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    author_user_id: &str,
    expected_ref: Option<&str>,
    request: CreateDraftRebaseRequest,
) -> Result<DraftRebaseResult, ServerError> {
    let row = sqlx::query(
        "SELECT project_id, author_user_id, base_commit_id, resource_scope,
                resource_kind, target_id, path, status, version, title, description
         FROM drafts WHERE draft_id = $1 FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    if row.try_get::<String, _>("author_user_id")? != author_user_id {
        return Err(ServerError::Forbidden(
            "only the draft author can rebase it".to_owned(),
        ));
    }
    let version: i64 = row.try_get("version")?;
    if version != request.expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            request.expected_draft_version,
            version,
        ));
    }
    let lifecycle: String = row.try_get("status")?;
    if lifecycle != "open" && lifecycle != "submitted" {
        return Err(ServerError::invalid_transition(
            "draft", &lifecycle, "rebased",
        ));
    }
    let candidate = load_reconciliation_candidate(tx, draft_id, &request.candidate_id).await?;
    if !candidate.valid
        || candidate.draft_version != version
        || candidate.base_commit_id != row.try_get::<Option<String>, _>("base_commit_id")?
    {
        return Err(ServerError::ReconciliationCandidateInvalid {
            candidate_id: request.candidate_id,
        });
    }
    let project_id: String = row.try_get("project_id")?;
    let scope = resource_scope(row.try_get::<String, _>("resource_scope")?.as_str())?;
    let current_ref = target_ref_for_draft(tx, &project_id, scope).await?;
    if current_ref.as_deref() != expected_ref {
        return Err(ServerError::precondition_failed(
            expected_ref,
            current_ref.as_deref(),
        ));
    }
    if candidate.current_commit_id != current_ref {
        return Err(ServerError::ReconciliationCandidateInvalid {
            candidate_id: request.candidate_id,
        });
    }
    let resolved_state = match (candidate.status, request.resolved_state) {
        (ReconciliationCandidateStatus::Clean, Some(_)) => {
            return Err(ServerError::InvalidRequest(
                "a clean candidate must be applied without resolved_state".to_owned(),
            ));
        }
        (ReconciliationCandidateStatus::Clean, None) => {
            candidate.proposed_state.clone().ok_or_else(|| {
                ServerError::InvalidRequest("clean candidate has no result".to_owned())
            })?
        }
        (ReconciliationCandidateStatus::Conflicts, Some(resolved)) => resolved,
        (ReconciliationCandidateStatus::Conflicts, None) => {
            return Err(ServerError::InvalidRequest(
                "a conflicts candidate requires a resolved_state".to_owned(),
            ));
        }
    };
    if resolved_state.resource.scope != scope
        || (resolved_state.exists && resolved_state.content.is_none())
    {
        return Err(ServerError::InvalidRequest(
            "resolved state does not match the draft resource".to_owned(),
        ));
    }
    if path_is_occupied(tx, current_ref.as_deref(), &resolved_state).await? {
        return Err(ServerError::InvalidRequest(
            "resolved state path is occupied in the current commit".to_owned(),
        ));
    }

    let previous_operations = load_draft_operations(tx, draft_id).await?;
    let previous_revision_id = prefixed_id("drv");
    sqlx::query(
        "INSERT INTO draft_revisions (
            revision_id, draft_id, draft_version, base_commit_id, lifecycle_status,
            title, description, operations
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&previous_revision_id)
    .bind(draft_id)
    .bind(version)
    .bind(row.try_get::<Option<String>, _>("base_commit_id")?)
    .bind(&lifecycle)
    .bind(row.try_get::<String, _>("title")?)
    .bind(row.try_get::<String, _>("description")?)
    .bind(Json(&previous_operations))
    .execute(&mut **tx)
    .await?;

    let operations = diff_resource_states(&candidate.current_state, &resolved_state);
    sqlx::query("DELETE FROM draft_operations WHERE draft_id = $1")
        .bind(draft_id)
        .execute(&mut **tx)
        .await?;
    for operation in operations {
        insert_draft_operation(tx, draft_id, operation).await?;
    }
    let next_version: i64 = sqlx::query_scalar(
        "UPDATE drafts
         SET base_commit_id = $2, version = version + 1, updated_at = now()
         WHERE draft_id = $1 RETURNING version",
    )
    .bind(draft_id)
    .bind(&current_ref)
    .fetch_one(&mut **tx)
    .await?;
    invalidate_draft_candidates(tx, draft_id).await?;
    let result_hash = state_hash(&resolved_state)?;
    let review_row = sqlx::query(
        "SELECT reviews.review_id, reviews.status, reviews.approved_result_hash
         FROM reviews
         JOIN review_drafts ON review_drafts.review_id = reviews.review_id
         WHERE review_drafts.draft_id = $1
         FOR UPDATE OF reviews",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?;
    let mut approval_invalidated = false;
    if let Some(review_row) = review_row {
        let status: String = review_row.try_get("status")?;
        let approved_hash: Option<String> = review_row.try_get("approved_result_hash")?;
        let review_id: String = review_row.try_get("review_id")?;
        let draft_ids = load_review_draft_ids(tx, &review_id).await?;
        let current_review_hash = review_result_hash(tx, &draft_ids).await?;
        let preserve_approval =
            status == "approved" && approved_hash.as_deref() == Some(&current_review_hash);
        approval_invalidated = status == "approved" && !preserve_approval;
        sqlx::query(
            "UPDATE reviews
             SET status = CASE WHEN status = 'approved' AND NOT $2 THEN 'open' ELSE status END,
                 approved_result_hash = CASE WHEN status = 'approved' AND $2 THEN approved_result_hash ELSE NULL END,
                 decision_body = CASE WHEN status = 'approved' AND $2 THEN decision_body ELSE NULL END,
                 decided_by_user_id = CASE WHEN status = 'approved' AND $2 THEN decided_by_user_id ELSE NULL END,
                 decided_at = CASE WHEN status = 'approved' AND $2 THEN decided_at ELSE NULL END,
                 version = version + 1, updated_at = now()
             WHERE review_id = $1",
        )
        .bind(&review_id)
        .bind(preserve_approval)
        .execute(&mut **tx)
        .await?;
    }
    let rebase_id = prefixed_id("rbs");
    sqlx::query(
        "INSERT INTO draft_rebases (
            rebase_id, draft_id, candidate_id, previous_revision_id,
            applied_by_user_id, resulting_draft_version, result_hash
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&rebase_id)
    .bind(draft_id)
    .bind(&candidate.candidate_id)
    .bind(&previous_revision_id)
    .bind(author_user_id)
    .bind(next_version)
    .bind(&result_hash)
    .execute(&mut **tx)
    .await?;
    insert_draft_event(
        tx,
        draft_id,
        &project_id,
        DraftEventType::Rebased,
        next_version,
        None,
    )
    .await?;
    let draft = load_draft_detail(tx, draft_id).await?;
    let review = match sqlx::query_scalar::<_, String>(
        "SELECT review_id FROM review_drafts WHERE draft_id = $1",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    {
        Some(review_id) => Some(load_review(tx, &review_id).await?),
        None => None,
    };
    Ok(DraftRebaseResult {
        rebase_id,
        previous_revision_id,
        draft,
        review,
        approval_invalidated,
    })
}

pub(crate) async fn draft_is_owned_by(
    pool: &PgPool,
    principal: &AuthPrincipal,
    draft_id: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM drafts d
            JOIN projects p ON p.project_id = d.project_id
            JOIN project_members m ON m.project_id = p.project_id
            WHERE d.draft_id = $1
              AND d.author_user_id = $2
              AND p.org_id = $3
              AND m.user_id = $2
         )",
    )
    .bind(draft_id)
    .bind(&principal.user_id)
    .bind(&principal.org_id)
    .fetch_one(pool)
    .await?)
}

pub(crate) async fn ensure_drafts_owned_by(
    tx: &mut Transaction<'_, Postgres>,
    principal: &AuthPrincipal,
    draft_ids: &[String],
) -> Result<(), ServerError> {
    let unauthorized_draft_id = sqlx::query_scalar::<_, String>(
        "SELECT requested.draft_id
         FROM unnest($1::TEXT[]) WITH ORDINALITY AS requested(draft_id, ordinal)
         WHERE NOT EXISTS (
             SELECT 1
             FROM drafts d
             JOIN projects p ON p.project_id = d.project_id
             JOIN project_members m ON m.project_id = p.project_id
             WHERE d.draft_id = requested.draft_id
               AND d.author_user_id = $2
               AND p.org_id = $3
               AND m.user_id = $2
         )
         ORDER BY requested.ordinal
         LIMIT 1",
    )
    .bind(draft_ids)
    .bind(&principal.user_id)
    .bind(&principal.org_id)
    .fetch_optional(&mut **tx)
    .await?;
    match unauthorized_draft_id {
        Some(draft_id) => Err(ServerError::not_found("draft", draft_id)),
        None => Ok(()),
    }
}

pub(crate) async fn ensure_drafts_authored_by(
    tx: &mut Transaction<'_, Postgres>,
    author_user_id: &str,
    draft_ids: &[String],
) -> Result<(), ServerError> {
    let owned_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(DISTINCT draft_id)
         FROM drafts
         WHERE draft_id = ANY($1) AND author_user_id = $2",
    )
    .bind(draft_ids)
    .bind(author_user_id)
    .fetch_one(&mut **tx)
    .await?;
    if owned_count == draft_ids.len() as i64 {
        Ok(())
    } else {
        Err(ServerError::Forbidden(
            "only the draft author can create its review".to_owned(),
        ))
    }
}

pub(crate) async fn create_draft(
    tx: &mut Transaction<'_, Postgres>,
    author_user_id: &str,
    mut request: CreateDraftRequest,
) -> Result<String, ServerError> {
    ensure_writable_draft_scope(request.resource.scope)?;
    if request.resource.scope == ResourceScope::Org {
        lock_org_draft_selection_coordination_for_project(tx, &request.project_id).await?;
    }
    let org_id = project_org_id(tx, &request.project_id).await?;
    user_ref(tx, author_user_id).await?;
    if let Some(base_commit_id) = request.base_commit_id.as_deref() {
        match request.resource.scope {
            ResourceScope::Org => validate_org_commit(tx, &org_id, base_commit_id).await?,
            ResourceScope::Project => {
                validate_project_commit(tx, &request.project_id, base_commit_id).await?
            }
        }
    }
    validate_draft_resource(&request.resource)?;
    for operation in &request.operations {
        validate_draft_operation_resource(&request.resource, operation)?;
    }
    validate_new_resource_draft_operations(&request.operations)?;
    if request.resource.scope == ResourceScope::Org {
        canonicalize_org_draft_targets_are_selected(
            tx,
            &request.project_id,
            &org_id,
            request.base_commit_id.as_deref(),
            &mut request.resource,
            &mut request.operations,
        )
        .await?;
    }

    let draft_id = prefixed_id("drf");
    sqlx::query(
        "INSERT INTO drafts (
                draft_id, project_id, author_user_id, title, description,
                resource_scope, resource_kind, base_commit_id, target_id, path, status, version,
                daemon_installation_id
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'open', 1, $11)",
    )
    .bind(&draft_id)
    .bind(&request.project_id)
    .bind(author_user_id)
    .bind(&request.title)
    .bind(request.description.as_deref().unwrap_or_default())
    .bind(request.resource.scope.as_str())
    .bind("memory")
    .bind(&request.base_commit_id)
    .bind(&request.resource.id)
    .bind(&request.resource.path)
    .bind(&request.daemon_installation_id)
    .execute(&mut **tx)
    .await?;

    for operation in request.operations {
        insert_draft_operation(tx, &draft_id, operation).await?;
    }
    insert_draft_event(
        tx,
        &draft_id,
        &request.project_id,
        DraftEventType::Created,
        1,
        Some(&request.daemon_installation_id),
    )
    .await?;

    Ok(draft_id)
}

pub(crate) async fn list_drafts(
    pool: &PgPool,
    author_user_id: &str,
    project_id: Option<&str>,
) -> Result<DraftListResponse, ServerError> {
    let rows = sqlx::query(
        "SELECT
            d.draft_id, d.project_id, d.base_commit_id, d.title, d.description,
            d.status, d.version, d.resource_scope, d.target_id, d.path,
            d.created_at, d.updated_at,
            u.user_id, u.email, u.display_name, u.avatar_url, u.role,
            current_ref.commit_id AS current_commit_id,
            candidate.status AS candidate_status,
            candidate.candidate_id,
            CASE
                WHEN d.base_commit_id IS NOT DISTINCT FROM current_ref.commit_id THEN FALSE
                WHEN base_entry.item_id IS NULL AND current_entry.item_id IS NULL THEN FALSE
                WHEN base_entry.item_id IS NOT NULL
                  AND current_entry.item_id IS NOT NULL
                  AND base_entry.item_id = current_entry.item_id
                  AND base_entry.path IS NOT DISTINCT FROM current_entry.path
                  AND base_entry.blob_id = current_entry.blob_id THEN FALSE
                ELSE TRUE
            END AS has_upstream_resource_changes
         FROM drafts d
         JOIN users u ON u.user_id = d.author_user_id
         JOIN projects p ON p.project_id = d.project_id
         JOIN refs current_ref
           ON current_ref.ref_name = 'refs/heads/main'
          AND (
              (d.resource_scope = 'org'
               AND current_ref.scope = 'org'
               AND current_ref.org_id = p.org_id)
              OR
              (d.resource_scope = 'project'
               AND current_ref.scope = 'project'
               AND current_ref.project_id = d.project_id)
          )
         LEFT JOIN LATERAL (
             SELECT operation.action
             FROM draft_operations operation
             WHERE operation.draft_id = d.draft_id
             ORDER BY operation.ordinal
             LIMIT 1
         ) first_operation ON TRUE
         LEFT JOIN LATERAL (
             SELECT e.item_id, e.path, e.blob_id
             FROM commits c
             JOIN tree_entries e ON e.tree_id = c.tree_id
             WHERE c.commit_id = d.base_commit_id
               AND d.base_commit_id IS DISTINCT FROM current_ref.commit_id
               AND e.resource_kind = 'memory'
               AND e.scope = d.resource_scope
               AND (
                   (d.target_id IS NOT NULL AND e.item_id = d.target_id)
                   OR
                   (d.target_id IS NULL
                    AND first_operation.action IS DISTINCT FROM 'create'
                    AND e.path = d.path)
               )
             ORDER BY e.item_id
             LIMIT 1
         ) base_entry ON TRUE
         LEFT JOIN LATERAL (
             SELECT e.item_id, e.path, e.blob_id
             FROM commits c
             JOIN tree_entries e ON e.tree_id = c.tree_id
             WHERE c.commit_id = current_ref.commit_id
               AND d.base_commit_id IS DISTINCT FROM current_ref.commit_id
               AND e.resource_kind = 'memory'
               AND e.scope = d.resource_scope
               AND (
                   (d.target_id IS NOT NULL AND e.item_id = d.target_id)
                   OR
                   (d.target_id IS NULL
                    AND first_operation.action IS DISTINCT FROM 'create'
                    AND e.path = d.path)
               )
             ORDER BY e.item_id
             LIMIT 1
         ) current_entry ON TRUE
         LEFT JOIN LATERAL (
             SELECT c.status, c.candidate_id
             FROM draft_reconciliation_candidates c
             WHERE c.draft_id = d.draft_id
               AND c.draft_version = d.version
               AND c.base_commit_id IS NOT DISTINCT FROM d.base_commit_id
               AND c.current_commit_id IS NOT DISTINCT FROM current_ref.commit_id
               AND c.invalidated_at IS NULL
               AND d.base_commit_id IS DISTINCT FROM current_ref.commit_id
             ORDER BY c.created_at DESC
             LIMIT 1
         ) candidate ON TRUE
         WHERE d.author_user_id = $1
           AND ($2::text IS NULL OR d.project_id = $2)
         ORDER BY d.updated_at DESC
         LIMIT 100",
    )
    .bind(author_user_id)
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    let items = rows
        .iter()
        .map(|row| {
            Ok(Draft {
                draft_id: row.try_get("draft_id")?,
                project_id: row.try_get("project_id")?,
                base_commit_id: row.try_get("base_commit_id")?,
                author: user_ref_from_row(row)?,
                title: row.try_get("title")?,
                description: row.try_get("description")?,
                resource: DraftResourceRef {
                    scope: resource_scope(row.try_get::<String, _>("resource_scope")?.as_str())?,
                    id: row.try_get("target_id")?,
                    path: row.try_get("path")?,
                },
                status: draft_status(row.try_get::<String, _>("status")?.as_str())?,
                coordination: draft_coordination_from_projection_row(row)?,
                version: row.try_get("version")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            })
        })
        .collect::<Result<Vec<_>, ServerError>>()?;
    Ok(DraftListResponse {
        items,
        page_info: page_info(),
    })
}

pub(crate) async fn update_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    expected_draft_version: i64,
    request: UpdateDraftRequest,
) -> Result<(), ServerError> {
    let row = sqlx::query(
        "SELECT title, description, status, version, project_id
             FROM drafts
             WHERE draft_id = $1
             FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let current_version: i64 = row.try_get("version")?;
    if current_version != expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            expected_draft_version,
            current_version,
        ));
    }
    let status = row.try_get::<String, _>("status")?;
    if status != "open" && status != "submitted" {
        return Err(ServerError::invalid_transition("draft", &status, "updated"));
    }
    let existing_title: String = row.try_get("title")?;
    let existing_description: String = row.try_get("description")?;
    let title = request.title.unwrap_or(existing_title);
    let description = request.description.unwrap_or(existing_description);
    let updated = sqlx::query(
        "UPDATE drafts
             SET title = $2, description = $3, version = version + 1, updated_at = now()
             WHERE draft_id = $1
             RETURNING project_id, version",
    )
    .bind(draft_id)
    .bind(title)
    .bind(description)
    .fetch_one(&mut **tx)
    .await?;
    invalidate_draft_candidates(tx, draft_id).await?;
    insert_draft_event(
        tx,
        draft_id,
        &updated.try_get::<String, _>("project_id")?,
        DraftEventType::Updated,
        updated.try_get("version")?,
        None,
    )
    .await?;
    Ok(())
}

pub(crate) async fn discard_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    actor_user_id: &str,
    expected_draft_version: i64,
) -> Result<DeleteResult, ServerError> {
    let row = sqlx::query(
        "SELECT project_id, status, version FROM drafts WHERE draft_id = $1 FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let version: i64 = row.try_get("version")?;
    if version != expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            expected_draft_version,
            version,
        ));
    }
    let status: String = row.try_get("status")?;
    if status != "open" && status != "submitted" {
        return Err(ServerError::invalid_transition(
            "draft",
            &status,
            "discarded",
        ));
    }
    let next_version: i64 = sqlx::query_scalar(
        "UPDATE drafts SET status = 'discarded', version = version + 1, updated_at = now()
             WHERE draft_id = $1 RETURNING version",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?;
    invalidate_draft_candidates(tx, draft_id).await?;
    sqlx::query(
        "UPDATE reviews
             SET status = 'rejected', version = version + 1,
                 decision_body = 'Draft discarded.', approved_result_hash = NULL,
                 decided_by_user_id = $2, decided_at = now(),
                 updated_at = now()
             WHERE draft_id = $1 AND status IN ('open', 'approved')",
    )
    .bind(draft_id)
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;
    insert_draft_event(
        tx,
        draft_id,
        &row.try_get::<String, _>("project_id")?,
        DraftEventType::Discarded,
        next_version,
        None,
    )
    .await?;
    Ok(DeleteResult {
        deleted: true,
        id: draft_id.to_owned(),
    })
}

pub(crate) async fn create_draft_operation_batch(
    tx: &mut Transaction<'_, Postgres>,
    request: DraftOperationBatchRequest,
) -> Result<DraftOperationBatchResponse, ServerError> {
    if request.operations.is_empty() {
        return Err(ServerError::InvalidRequest(
            "draft operation batch cannot be empty".to_owned(),
        ));
    }
    let draft_ids = request
        .operations
        .iter()
        .map(|item| item.draft_id.clone())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT DISTINCT project.org_id
             FROM drafts AS draft
             JOIN projects AS project ON project.project_id = draft.project_id
             WHERE draft.draft_id = ANY($1)
               AND draft.resource_scope = 'org'
             ORDER BY project.org_id",
    )
    .bind(&draft_ids)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        lock_org_draft_selection_coordination(tx, &row.try_get::<String, _>("org_id")?).await?;
    }
    let mut accepted_operations = Vec::new();
    let mut cursor = None;
    let daemon_installation_id = request.daemon_installation_id;
    for item in request.operations {
        cursor = Some(
            append_draft_operation_in_tx(
                tx,
                &item.draft_id,
                item.expected_draft_version,
                item.operation,
                Some(&daemon_installation_id),
                true,
            )
            .await?,
        );
        accepted_operations.push(item.local_operation_id);
    }
    Ok(DraftOperationBatchResponse {
        cursor: cursor.expect("non-empty batch").to_string(),
        accepted_operations,
    })
}

pub(crate) async fn list_draft_events(
    pool: &PgPool,
    author_user_id: &str,
    after_cursor: Option<&str>,
    limit: Option<i64>,
) -> Result<DraftEventListResponse, ServerError> {
    let limit = limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ServerError::InvalidRequest(
            "draft event limit must be between 1 and 200".to_owned(),
        ));
    }
    let fetch_limit = limit + 1;
    let mut rows = if let Some(after_cursor) = after_cursor {
        let after_sequence = after_cursor
            .parse::<i64>()
            .map_err(|_| ServerError::InvalidRequest("invalid draft event cursor".to_owned()))?;
        sqlx::query(
            "SELECT e.server_sequence, e.event_id, e.draft_id, e.project_id, e.event_type,
                    e.version, e.daemon_installation_id, e.created_at
             FROM draft_events e
             JOIN drafts d ON d.draft_id = e.draft_id
             WHERE e.server_sequence > $1 AND d.author_user_id = $2
             ORDER BY e.server_sequence
             LIMIT $3",
        )
        .bind(after_sequence)
        .bind(author_user_id)
        .bind(fetch_limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT e.server_sequence, e.event_id, e.draft_id, e.project_id, e.event_type,
                    e.version, e.daemon_installation_id, e.created_at
             FROM draft_events e
             JOIN drafts d ON d.draft_id = e.draft_id
             WHERE d.author_user_id = $1
             ORDER BY e.server_sequence
             LIMIT $2",
        )
        .bind(author_user_id)
        .bind(fetch_limit)
        .fetch_all(pool)
        .await?
    };
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = rows
        .last()
        .map(|row| {
            row.try_get::<i64, _>("server_sequence")
                .map(|value| value.to_string())
        })
        .transpose()?;
    let events = rows
        .iter()
        .map(draft_event_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DraftEventListResponse {
        next_cursor,
        has_more,
        events,
    })
}

pub(crate) fn draft_event_from_row(row: &sqlx::postgres::PgRow) -> Result<DraftEvent, ServerError> {
    Ok(DraftEvent {
        event_id: row.try_get("event_id")?,
        draft_id: row.try_get("draft_id")?,
        project_id: row.try_get("project_id")?,
        event_type: draft_event_type(row.try_get::<String, _>("event_type")?.as_str())?,
        version: row.try_get("version")?,
        daemon_installation_id: row.try_get("daemon_installation_id")?,
        created_at: row.try_get("created_at")?,
    })
}

pub(crate) fn draft_coordination_from_projection_row(
    row: &sqlx::postgres::PgRow,
) -> Result<DraftCoordination, ServerError> {
    let base_commit_id: Option<String> = row.try_get("base_commit_id")?;
    let current_commit_id: Option<String> = row.try_get("current_commit_id")?;
    let freshness = if base_commit_id == current_commit_id {
        DraftFreshness::Current
    } else {
        DraftFreshness::Behind
    };
    let candidate_status: Option<String> = row.try_get("candidate_status")?;
    let reconciliation = if freshness == DraftFreshness::Current {
        DraftReconciliationStatus::Unknown
    } else {
        match candidate_status.as_deref() {
            Some("clean") => DraftReconciliationStatus::Clean,
            Some("conflicts") => DraftReconciliationStatus::Conflicts,
            Some(status) => {
                return Err(ServerError::InvalidRequest(format!(
                    "unknown reconciliation status: {status}"
                )));
            }
            None => DraftReconciliationStatus::Unknown,
        }
    };
    Ok(DraftCoordination {
        freshness,
        current_commit_id,
        has_upstream_resource_changes: row.try_get("has_upstream_resource_changes")?,
        reconciliation,
        candidate_id: row.try_get("candidate_id")?,
    })
}
