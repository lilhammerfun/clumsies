use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::config::{
    META_DRAFT_EVENTS_CURSOR, META_DRAFT_SYNC_LAST_ATTEMPT_AT, META_DRAFT_SYNC_LAST_SUCCESS_AT,
};
use crate::state::DaemonState;
use crate::types::{
    ApiError, DaemonContentDraftUpdate, DaemonCreateDraftOperation, DaemonDeleteDraftOperation,
    DaemonDiscardDraftOperation, DaemonDraftDetail, DaemonDraftFreshness, DaemonDraftListQuery,
    DaemonDraftListResponse, DaemonDraftOperation, DaemonDraftOperationRecordSource,
    DaemonDraftReconciliationStatus, DaemonDraftResourceKind, DaemonDraftScope, DaemonDraftSummary,
    DaemonError, DaemonLocalDraftOperation, DaemonLocalDraftStatus, DaemonRenameDraftOperation,
    DaemonSyncStatus, DaemonUpdateDraftOperation, DraftOperationSyncStatus, DraftSyncError,
    QueuedDraftOperation, ServerCreateDraftRequest, ServerDraftEventListResponse,
    ServerDraftMutationResponse, ServerDraftOperationAction, ServerDraftOperationBatchItem,
    ServerDraftOperationBatchRequest, ServerDraftOperationBatchResponse, ServerDraftOperationInput,
    ServerDraftProjectionDetail, ServerDraftProjectionOperation, ServerDraftResourceRef,
    SyncChannelStatus, SyncState,
};
use crate::{commit_sync, delete_server_json, get_server_json, load_meta_value, post_server_json};

const MAX_CONCURRENT_DRAFT_PROJECTION_REQUESTS: usize = 8;

/// Marks event streams to replay when a project's membership returns.
const DEFERRED_EVENTS_PREFIX: &str = "draft_events_deferred:";

/// Resume events skipped while their project was inaccessible, including after restart.
///
/// # Errors
/// Propagates metadata failures without advancing the event cursor.
pub(crate) async fn resume_deferred_events(
    state: &DaemonState,
    project_ids: &BTreeSet<String>,
) -> Result<(), DaemonError> {
    let keys: Vec<String> = project_ids
        .iter()
        .map(|id| format!("{DEFERRED_EVENTS_PREFIX}{id}"))
        .collect();
    let mut tx = state.inner.pool.begin().await?;
    let resumed =
        sqlx::query("DELETE FROM daemon_meta WHERE key IN (SELECT value FROM json_each($1))")
            .bind(serde_json::to_string(&keys)?)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if resumed > 0 {
        // ponytail: replay the author's feed once on restored access; add per-project cursors if large histories make recovery slow.
        sqlx::query("DELETE FROM daemon_meta WHERE key = $1")
            .bind(META_DRAFT_EVENTS_CURSOR)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
struct DraftListCursor {
    updated_at: String,
    created_at: String,
    draft_id: String,
}

pub(crate) struct LocalDraftResolutionInput<'a> {
    pub(crate) requested_draft_id: Option<&'a str>,
    pub(crate) project_id: &'a str,
    pub(crate) requested_base_commit_id: Option<&'a str>,
    pub(crate) new_draft_base_commit_id: Option<&'a str>,
    pub(crate) scope: DaemonDraftScope,
    pub(crate) resource: DaemonDraftResourceKind,
    pub(crate) op: &'a mut DaemonDraftOperation,
}

pub(crate) async fn resolve_local_draft(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: LocalDraftResolutionInput<'_>,
) -> Result<String, DaemonError> {
    let LocalDraftResolutionInput {
        requested_draft_id,
        project_id,
        requested_base_commit_id,
        new_draft_base_commit_id,
        scope,
        resource,
        op,
    } = input;
    if let Some(draft_id) = requested_draft_id {
        let row = sqlx::query(
            "SELECT project_id, resource_scope, resource_kind, base_commit_id, status
             FROM local_drafts
             WHERE draft_id = $1",
        )
        .bind(draft_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("local draft not found: {draft_id}")))?;
        let stored_scope: String = row.try_get("resource_scope")?;
        let stored_kind: String = row.try_get("resource_kind")?;
        let stored_project_id: String = row.try_get("project_id")?;
        if stored_project_id != project_id
            || stored_scope != scope.as_str()
            || stored_kind != resource.as_str()
        {
            return Err(DaemonError::InvalidRequest(format!(
                "local draft {draft_id} belongs to a different project, scope, or resource kind"
            )));
        }
        let stored_base_commit_id: Option<String> = row.try_get("base_commit_id")?;
        if requested_base_commit_id.is_some()
            && stored_base_commit_id.as_deref() != requested_base_commit_id
        {
            return Err(DaemonError::InvalidRequest(format!(
                "local draft {draft_id} has a different base commit"
            )));
        }
        let status: String = row.try_get("status")?;
        if status != "open" && status != "submitted" {
            return Err(DaemonError::InvalidRequest(format!(
                "local draft {draft_id} is {status}"
            )));
        }
        normalize_unpublished_delete(tx, draft_id, op).await?;
        if op.discard.is_some() {
            mark_local_draft_discarded(tx, draft_id).await?;
        }
        if let Some(create) = &op.create {
            sqlx::query("UPDATE local_drafts SET path = $2 WHERE draft_id = $1")
                .bind(draft_id)
                .bind(&create.path)
                .execute(&mut **tx)
                .await?;
        }
        return Ok(draft_id.to_owned());
    }

    if let Some(create) = &op.create {
        let draft_id = format!("draft_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO local_drafts (
                draft_id, project_id, base_commit_id, current_commit_id,
                resource_scope, resource_kind, target_id, path, status
             )
             VALUES ($1, $2, $3, $3, $4, $5, NULL, $6, 'open')",
        )
        .bind(&draft_id)
        .bind(project_id)
        .bind(new_draft_base_commit_id)
        .bind(scope.as_str())
        .bind(resource.as_str())
        .bind(&create.path)
        .execute(&mut **tx)
        .await?;
        return Ok(draft_id);
    }

    let target_id = op
        .target_id()
        .ok_or_else(|| DaemonError::InvalidRequest("operation target id is required".to_owned()))?;
    if let Some(existing) = sqlx::query(
        "SELECT draft_id, project_id, resource_scope, resource_kind, base_commit_id
         FROM local_drafts
         WHERE (draft_id = $1 OR target_id = $1)
           AND project_id = $2
           AND resource_scope = $3
           AND resource_kind = $4
           AND status IN ('open', 'submitted')
         ORDER BY updated_at DESC
         LIMIT 1",
    )
    .bind(target_id)
    .bind(project_id)
    .bind(scope.as_str())
    .bind(resource.as_str())
    .fetch_optional(&mut **tx)
    .await?
    {
        let draft_id: String = existing.try_get("draft_id")?;
        let stored_project_id: String = existing.try_get("project_id")?;
        let stored_kind: String = existing.try_get("resource_kind")?;
        if stored_project_id != project_id || stored_kind != resource.as_str() {
            return Err(DaemonError::InvalidRequest(format!(
                "local draft {draft_id} belongs to a different project or resource kind"
            )));
        }
        let stored_base_commit_id: Option<String> = existing.try_get("base_commit_id")?;
        if requested_base_commit_id.is_some()
            && stored_base_commit_id.as_deref() != requested_base_commit_id
        {
            return Err(DaemonError::InvalidRequest(format!(
                "local draft {draft_id} has a different base commit"
            )));
        }
        normalize_unpublished_delete(tx, &draft_id, op).await?;
        if op.discard.is_some() {
            mark_local_draft_discarded(tx, &draft_id).await?;
        }
        return Ok(draft_id);
    }

    let draft_id = format!("draft_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO local_drafts (
            draft_id, project_id, base_commit_id, current_commit_id,
            resource_scope, resource_kind, target_id, path, status
         )
         VALUES ($1, $2, $3, $3, $4, $5, $6, NULL, $7)",
    )
    .bind(&draft_id)
    .bind(project_id)
    .bind(new_draft_base_commit_id)
    .bind(scope.as_str())
    .bind(resource.as_str())
    .bind(target_id)
    .bind(if op.discard.is_some() {
        "discarded"
    } else {
        "open"
    })
    .execute(&mut **tx)
    .await?;
    Ok(draft_id)
}

pub(crate) async fn normalize_unpublished_delete(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    draft_id: &str,
    operation: &mut DaemonDraftOperation,
) -> Result<(), DaemonError> {
    if operation.delete.is_none() {
        return Ok(());
    }
    let creates_resource: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1
            FROM local_draft_operations
            WHERE draft_id = $1
              AND json_type(operation_json, '$.create') = 'object'
         )",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?;
    if creates_resource {
        operation.delete = None;
        operation.discard = Some(DaemonDiscardDraftOperation {
            id: draft_id.to_owned(),
        });
    }
    Ok(())
}

pub(crate) async fn mark_local_draft_discarded(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    draft_id: &str,
) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE local_drafts
         SET status = 'discarded', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE draft_id = $1",
    )
    .bind(draft_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn list_local_drafts(
    pool: &SqlitePool,
    query: DaemonDraftListQuery,
) -> Result<DaemonDraftListResponse, DaemonError> {
    let resource_kind = query
        .resource
        .map(|value| draft_resource_kind_from_str(value.as_str()).map(|kind| kind.as_str()))
        .transpose()?
        .map(ToOwned::to_owned);
    let status = query
        .status
        .map(|value| local_draft_status_from_str(value.as_str()).map(|status| status.as_str()))
        .transpose()?
        .map(ToOwned::to_owned);
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let cursor = query
        .cursor
        .as_deref()
        .map(decode_draft_list_cursor)
        .transpose()?;
    let rows = sqlx::query(
        "SELECT
            d.draft_id, d.project_id, d.server_draft_id, d.server_version, d.base_commit_id,
            d.current_commit_id, d.freshness, d.has_upstream_resource_changes,
            d.reconciliation, d.reconciliation_candidate_id,
            d.resource_scope, d.resource_kind, d.target_id, d.path,
            d.status, d.created_at, d.updated_at,
            (
                SELECT COUNT(*)
                FROM local_draft_operations o
                WHERE o.draft_id = d.draft_id AND o.sync_status IN ('queued', 'syncing', 'retrying')
            ) AS pending_operation_count,
            (
                SELECT COUNT(*)
                FROM local_draft_operations o
                WHERE o.draft_id = d.draft_id AND o.sync_status = 'failed'
            ) AS failed_operation_count
         FROM local_drafts d
         WHERE ($1 IS NULL OR d.resource_kind = $1)
           AND ($2 IS NULL OR d.status = $2)
           AND (
                $3 IS NULL
                OR d.updated_at < $3
                OR (d.updated_at = $3 AND d.created_at < $4)
                OR (d.updated_at = $3 AND d.created_at = $4 AND d.draft_id > $5)
           )
         ORDER BY d.updated_at DESC, d.created_at DESC, d.draft_id ASC
         LIMIT $6",
    )
    .bind(resource_kind.as_deref())
    .bind(status.as_deref())
    .bind(cursor.as_ref().map(|cursor| cursor.updated_at.as_str()))
    .bind(cursor.as_ref().map(|cursor| cursor.created_at.as_str()))
    .bind(cursor.as_ref().map(|cursor| cursor.draft_id.as_str()))
    .bind(limit + 1)
    .fetch_all(pool)
    .await?;

    let has_more = rows.len() as i64 > limit;
    let items = rows
        .iter()
        .take(limit as usize)
        .map(local_draft_summary_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if has_more {
        items
            .last()
            .map(|draft| {
                encode_draft_list_cursor(&DraftListCursor {
                    updated_at: draft.updated_at.clone(),
                    created_at: draft.created_at.clone(),
                    draft_id: draft.draft_id.clone(),
                })
            })
            .transpose()?
    } else {
        None
    };
    Ok(DaemonDraftListResponse { items, next_cursor })
}

fn encode_draft_list_cursor(cursor: &DraftListCursor) -> Result<String, DaemonError> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(cursor)?))
}

fn decode_draft_list_cursor(cursor: &str) -> Result<DraftListCursor, DaemonError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| DaemonError::InvalidRequest("Invalid Draft list cursor".to_owned()))?;
    serde_json::from_slice(&decoded)
        .map_err(|_| DaemonError::InvalidRequest("Invalid Draft list cursor".to_owned()))
}

pub(crate) async fn load_local_draft_detail(
    pool: &SqlitePool,
    draft_id: &str,
) -> Result<DaemonDraftDetail, DaemonError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT
            d.draft_id, d.project_id, d.server_draft_id, d.server_version, d.base_commit_id,
            d.current_commit_id, d.freshness, d.has_upstream_resource_changes,
            d.reconciliation, d.reconciliation_candidate_id,
            d.resource_scope, d.resource_kind, d.target_id, d.path,
            d.status, d.created_at, d.updated_at,
            (
                SELECT COUNT(*)
                FROM local_draft_operations o
                WHERE o.draft_id = d.draft_id AND o.sync_status IN ('queued', 'syncing', 'retrying')
            ) AS pending_operation_count,
            (
                SELECT COUNT(*)
                FROM local_draft_operations o
                WHERE o.draft_id = d.draft_id AND o.sync_status = 'failed'
            ) AS failed_operation_count
         FROM local_drafts d
         WHERE d.draft_id = $1",
    )
    .bind(draft_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DaemonError::NotFound(format!("local draft not found: {draft_id}")))?;
    let draft = local_draft_summary_from_row(&row)?;
    let rows = sqlx::query(
        "SELECT
            local_operation_id, resource_kind, operation_json, source, sync_status,
            last_error, created_at, updated_at
         FROM local_draft_operations
         WHERE draft_id = $1
         ORDER BY rowid ASC",
    )
    .bind(draft_id)
    .fetch_all(&mut *tx)
    .await?;
    let operations = rows
        .iter()
        .map(local_draft_operation_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    tx.commit().await?;
    Ok(DaemonDraftDetail { draft, operations })
}

pub(crate) fn local_draft_summary_from_row(
    row: &SqliteRow,
) -> Result<DaemonDraftSummary, DaemonError> {
    Ok(DaemonDraftSummary {
        draft_id: row.try_get("draft_id")?,
        project_id: row.try_get("project_id")?,
        server_draft_id: row.try_get("server_draft_id")?,
        server_version: row.try_get("server_version")?,
        base_commit_id: row.try_get("base_commit_id")?,
        current_commit_id: row.try_get("current_commit_id")?,
        freshness: draft_freshness_from_str(row.try_get::<String, _>("freshness")?.as_str())?,
        has_upstream_resource_changes: row.try_get("has_upstream_resource_changes")?,
        reconciliation: draft_reconciliation_status_from_str(
            row.try_get::<String, _>("reconciliation")?.as_str(),
        )?,
        reconciliation_candidate_id: row.try_get("reconciliation_candidate_id")?,
        scope: daemon_draft_scope_from_str(row.try_get::<String, _>("resource_scope")?.as_str())?,
        resource_kind: draft_resource_kind_from_str(
            row.try_get::<String, _>("resource_kind")?.as_str(),
        )?,
        target_id: row.try_get("target_id")?,
        path: row.try_get("path")?,
        status: local_draft_status_from_str(row.try_get::<String, _>("status")?.as_str())?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        pending_operation_count: row.try_get("pending_operation_count")?,
        failed_operation_count: row.try_get("failed_operation_count")?,
    })
}

pub(crate) fn local_draft_operation_from_row(
    row: &SqliteRow,
) -> Result<DaemonLocalDraftOperation, DaemonError> {
    Ok(DaemonLocalDraftOperation {
        local_operation_id: row.try_get("local_operation_id")?,
        resource_kind: draft_resource_kind_from_str(
            row.try_get::<String, _>("resource_kind")?.as_str(),
        )?,
        operation: serde_json::from_str(&row.try_get::<String, _>("operation_json")?)?,
        source: draft_operation_source_from_str(row.try_get::<String, _>("source")?.as_str())?,
        sync_status: draft_operation_sync_status_from_str(
            row.try_get::<String, _>("sync_status")?.as_str(),
        )?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(crate) async fn load_sync_status(state: &DaemonState) -> Result<DaemonSyncStatus, DaemonError> {
    load_sync_status_scoped(state, None).await
}

pub(crate) async fn load_project_sync_status(
    state: &DaemonState,
    project_id: &str,
) -> Result<DaemonSyncStatus, DaemonError> {
    load_sync_status_scoped(state, Some(project_id)).await
}

async fn load_sync_status_scoped(
    state: &DaemonState,
    project_id: Option<&str>,
) -> Result<DaemonSyncStatus, DaemonError> {
    let pool = &state.inner.pool;
    let accessible = crate::project_access::project_ids_json(state);
    let unavailable_projects = crate::project_access::unavailable(state)
        .await?
        .into_iter()
        .filter(|project| project_id.is_none_or(|id| project.project_id == id))
        .collect();
    let pending_operation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM local_draft_operations AS o
         JOIN local_drafts AS d ON d.draft_id = o.draft_id
         WHERE o.sync_status IN ('queued', 'syncing', 'retrying')
           AND ($1 IS NULL OR d.project_id = $1)
           AND ($2 IS NULL OR d.project_id IN (SELECT value FROM json_each($2)))",
    )
    .bind(project_id)
    .bind(&accessible)
    .fetch_one(pool)
    .await?;
    let failed_operation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM local_draft_operations AS o
         JOIN local_drafts AS d ON d.draft_id = o.draft_id
         WHERE o.sync_status = 'failed'
           AND ($1 IS NULL OR d.project_id = $1)
           AND ($2 IS NULL OR d.project_id IN (SELECT value FROM json_each($2)))",
    )
    .bind(project_id)
    .bind(&accessible)
    .fetch_one(pool)
    .await?;
    let retrying_operation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM local_draft_operations AS o
         JOIN local_drafts AS d ON d.draft_id = o.draft_id
         WHERE o.sync_status = 'retrying'
           AND ($1 IS NULL OR d.project_id = $1)
           AND ($2 IS NULL OR d.project_id IN (SELECT value FROM json_each($2)))",
    )
    .bind(project_id)
    .bind(&accessible)
    .fetch_one(pool)
    .await?;
    let behind_draft_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM local_drafts
         WHERE freshness = 'behind' AND ($1 IS NULL OR project_id = $1)
           AND ($2 IS NULL OR project_id IN (SELECT value FROM json_each($2)))",
    )
    .bind(project_id)
    .bind(&accessible)
    .fetch_one(pool)
    .await?;
    let reconciliation_conflict_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM local_drafts
         WHERE reconciliation = 'conflicts' AND ($1 IS NULL OR project_id = $1)
           AND ($2 IS NULL OR project_id IN (SELECT value FROM json_each($2)))",
    )
    .bind(project_id)
    .bind(&accessible)
    .fetch_one(pool)
    .await?;
    let server_cursor = load_meta_value(pool, META_DRAFT_EVENTS_CURSOR).await?;
    let last_attempt_key = draft_sync_meta_key(META_DRAFT_SYNC_LAST_ATTEMPT_AT, project_id);
    let last_success_key = draft_sync_meta_key(META_DRAFT_SYNC_LAST_SUCCESS_AT, project_id);
    let last_attempt_at = load_meta_value(pool, &last_attempt_key).await?;
    let last_success_at = load_meta_value(pool, &last_success_key).await?;
    let last_error: Option<String> = sqlx::query_scalar(
        "SELECT o.last_error
         FROM local_draft_operations AS o
         JOIN local_drafts AS d ON d.draft_id = o.draft_id
         WHERE o.sync_status IN ('retrying', 'failed') AND o.last_error IS NOT NULL
           AND ($1 IS NULL OR d.project_id = $1)
           AND ($2 IS NULL OR d.project_id IN (SELECT value FROM json_each($2)))
         ORDER BY o.updated_at DESC
         LIMIT 1",
    )
    .bind(project_id)
    .bind(&accessible)
    .fetch_optional(pool)
    .await?;
    let readiness = state.project_config().server_readiness();
    let config_error = (!readiness.ready
        && state.inner.config.sync.enabled
        && pending_operation_count > 0)
        .then(|| ApiError {
            code: "daemon_project_config_incomplete".to_owned(),
            message: format!(
                "Daemon project config is missing required fields: {}",
                readiness.missing_fields.join(", ")
            ),
            request_id: "local".to_owned(),
            details: json!({ "missing_fields": readiness.missing_fields }),
        });
    let draft_state = if config_error.is_some() {
        SyncState::Degraded
    } else if failed_operation_count > 0 {
        SyncState::Failed
    } else if retrying_operation_count > 0 {
        SyncState::Retrying
    } else if pending_operation_count > 0 {
        SyncState::Queued
    } else {
        SyncState::Idle
    };
    let commit_sync = match project_id {
        Some(project_id) => commit_sync::status_for_project(state, project_id).await?,
        None => commit_sync::status(state).await?,
    };
    let overall_last_success_at = match (&last_success_at, &commit_sync.last_success_at) {
        (Some(draft), Some(commit)) => Some(std::cmp::max(draft, commit).clone()),
        (Some(draft), None) => Some(draft.clone()),
        (None, Some(commit)) => Some(commit.clone()),
        (None, None) => None,
    };

    Ok(DaemonSyncStatus {
        unavailable_projects,
        draft_sync: SyncChannelStatus {
            state: draft_state,
            server_cursor,
            last_attempt_at,
            last_success_at: last_success_at.clone(),
            last_error: config_error.or_else(|| {
                last_error.map(|message| ApiError {
                    code: if retrying_operation_count > 0 {
                        "draft_sync_retrying".to_owned()
                    } else {
                        "draft_sync_failed".to_owned()
                    },
                    message,
                    request_id: "local".to_owned(),
                    details: json!({}),
                })
            }),
        },
        commit_sync,
        pending_operation_count,
        failed_operation_count,
        behind_draft_count,
        reconciliation_conflict_count,
        last_success_at: overall_last_success_at,
    })
}

pub(crate) fn draft_sync_meta_key(base: &str, project_id: Option<&str>) -> String {
    match project_id {
        Some(project_id) => format!("{base}:{project_id}"),
        None => base.to_owned(),
    }
}

pub(crate) async fn drain_draft_queue(
    state: &DaemonState,
    project_id: Option<&str>,
) -> Result<bool, DaemonError> {
    let accessible = crate::project_access::project_ids_json(state);
    let mut queue_converged = true;
    loop {
        let Some(operation) =
            load_next_queued_operation(&state.inner.pool, project_id, accessible.as_deref())
                .await?
        else {
            break;
        };
        crate::project_access::require_current(state)?;
        mark_operation_syncing(&state.inner.pool, &operation.local_operation_id).await?;
        if let Err(error) = sync_one_draft_operation(state, operation).await {
            queue_converged = false;
            if error.is_retryable() {
                mark_operation_retrying(
                    &state.inner.pool,
                    error.local_operation_id(),
                    &error.to_string(),
                )
                .await?;
            } else {
                mark_operation_failed(
                    &state.inner.pool,
                    error.local_operation_id(),
                    &error.to_string(),
                )
                .await?;
            }
        }
    }
    let unsynced_operation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM local_draft_operations AS o
         JOIN local_drafts AS d ON d.draft_id = o.draft_id
         WHERE o.sync_status != 'synced'
           AND ($1 IS NULL OR d.project_id = $1)
           AND ($2 IS NULL OR d.project_id IN (SELECT value FROM json_each($2)))",
    )
    .bind(project_id)
    .bind(&accessible)
    .fetch_one(&state.inner.pool)
    .await?;
    Ok(queue_converged && unsynced_operation_count == 0)
}

pub(crate) async fn sync_one_draft_operation(
    state: &DaemonState,
    operation: QueuedDraftOperation,
) -> Result<(), DraftSyncError> {
    let local_operation_id = operation.local_operation_id.clone();
    let draft_operation: DaemonDraftOperation = serde_json::from_str(&operation.operation_json)
        .map_err(|error| DraftSyncError::new(local_operation_id.clone(), error.to_string()))?;
    if draft_operation.discard.is_some() {
        if let Some(server_draft_id) = operation.server_draft_id.as_deref() {
            delete_server_json(
                state,
                &format!("/api/v1/drafts/{server_draft_id}"),
                operation.server_version,
            )
            .await
            .map_err(|error| {
                DraftSyncError::from_daemon_error(local_operation_id.clone(), error)
            })?;
        }
        mark_operation_synced(&state.inner.pool, &local_operation_id)
            .await
            .map_err(|error| DraftSyncError::new(local_operation_id.clone(), error.to_string()))?;
        return Ok(());
    }
    let Some(server_operation) =
        map_daemon_operation_to_server(operation.scope, operation.resource_kind, &draft_operation)
            .map_err(|error| DraftSyncError::new(local_operation_id.clone(), error.to_string()))?
    else {
        mark_operation_synced(&state.inner.pool, &local_operation_id)
            .await
            .map_err(|error| DraftSyncError::new(local_operation_id.clone(), error.to_string()))?;
        return Ok(());
    };

    if let Some(server_draft_id) = operation.server_draft_id {
        let request = ServerDraftOperationBatchRequest {
            daemon_installation_id: state.inner.daemon_installation_id.clone(),
            operations: vec![ServerDraftOperationBatchItem {
                local_operation_id: local_operation_id.clone(),
                draft_id: server_draft_id,
                expected_draft_version: operation.server_version,
                operation: server_operation,
            }],
        };
        let response: ServerDraftOperationBatchResponse =
            post_server_json(state, "/api/v1/draft-operation-batches", &request)
                .await
                .map_err(|error| {
                    DraftSyncError::from_daemon_error(local_operation_id.clone(), error)
                })?;
        if !response
            .accepted_operations
            .iter()
            .any(|accepted| accepted == &local_operation_id)
        {
            return Err(DraftSyncError::new(
                local_operation_id,
                "Server did not accept local operation",
            ));
        }
        mark_batch_operation_synced(
            &state.inner.pool,
            &operation.draft_id,
            &local_operation_id,
            operation.server_version + 1,
        )
        .await
        .map_err(|error| DraftSyncError::new(local_operation_id.clone(), error.to_string()))?;
        return Ok(());
    }

    let request = ServerCreateDraftRequest {
        daemon_installation_id: state.inner.daemon_installation_id.clone(),
        project_id: operation.project_id.clone(),
        base_commit_id: operation.base_commit_id.clone(),
        title: draft_title(&operation),
        description: None,
        resource: server_operation.resource.clone(),
        operations: vec![server_operation],
    };
    let response: ServerDraftMutationResponse = post_server_json(state, "/api/v1/drafts", &request)
        .await
        .map_err(|error| DraftSyncError::from_daemon_error(local_operation_id.clone(), error))?;
    mark_initial_operation_synced(
        &state.inner.pool,
        &operation.draft_id,
        &local_operation_id,
        &response.draft.draft_id,
        response.draft.version,
    )
    .await
    .map_err(|error| DraftSyncError::new(local_operation_id.clone(), error.to_string()))?;
    Ok(())
}

pub(crate) async fn load_next_queued_operation(
    pool: &SqlitePool,
    project_id: Option<&str>,
    accessible: Option<&str>,
) -> Result<Option<QueuedDraftOperation>, DaemonError> {
    let Some(row) = sqlx::query(
        "SELECT
            o.local_operation_id, o.draft_id, o.resource_kind, o.operation_json,
            d.project_id, d.resource_scope, d.server_draft_id, d.server_version,
            d.base_commit_id, d.target_id, d.path
         FROM local_draft_operations o
         JOIN local_drafts d ON d.draft_id = o.draft_id
         WHERE o.sync_status = 'queued'
           AND ($1 IS NULL OR d.project_id = $1)
           AND ($2 IS NULL OR d.project_id IN (SELECT value FROM json_each($2)))
           AND NOT EXISTS (
               SELECT 1
               FROM local_draft_operations AS prior
               WHERE prior.draft_id = o.draft_id
                 AND prior.rowid < o.rowid
                 AND prior.sync_status != 'synced'
           )
         ORDER BY o.created_at, o.rowid
         LIMIT 1",
    )
    .bind(project_id)
    .bind(accessible)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    Ok(Some(QueuedDraftOperation {
        local_operation_id: row.try_get("local_operation_id")?,
        draft_id: row.try_get("draft_id")?,
        project_id: row.try_get("project_id")?,
        scope: daemon_draft_scope_from_str(row.try_get::<String, _>("resource_scope")?.as_str())?,
        resource_kind: draft_resource_kind_from_str(
            row.try_get::<String, _>("resource_kind")?.as_str(),
        )?,
        operation_json: row.try_get("operation_json")?,
        server_draft_id: row.try_get("server_draft_id")?,
        server_version: row.try_get("server_version")?,
        base_commit_id: row.try_get("base_commit_id")?,
        target_id: row.try_get("target_id")?,
        path: row.try_get("path")?,
    }))
}

pub(crate) async fn mark_operation_syncing(
    pool: &SqlitePool,
    local_operation_id: &str,
) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE local_draft_operations
         SET sync_status = 'syncing', last_error = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE local_operation_id = $1",
    )
    .bind(local_operation_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn mark_operation_failed(
    pool: &SqlitePool,
    local_operation_id: &str,
    message: &str,
) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE local_draft_operations
         SET sync_status = 'failed', last_error = $2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE local_operation_id = $1",
    )
    .bind(local_operation_id)
    .bind(message)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn mark_operation_retrying(
    pool: &SqlitePool,
    local_operation_id: &str,
    message: &str,
) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE local_draft_operations
         SET sync_status = 'retrying', last_error = $2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE local_operation_id = $1",
    )
    .bind(local_operation_id)
    .bind(message)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn queue_retrying_operations(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE local_draft_operations
         SET sync_status = 'queued', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE sync_status = 'retrying'",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn recover_interrupted_operations(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE local_draft_operations
         SET sync_status = 'queued', last_error = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE sync_status = 'syncing'",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn mark_operation_synced(
    pool: &SqlitePool,
    local_operation_id: &str,
) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE local_draft_operations
         SET sync_status = 'synced', last_error = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE local_operation_id = $1",
    )
    .bind(local_operation_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn mark_initial_operation_synced(
    pool: &SqlitePool,
    draft_id: &str,
    local_operation_id: &str,
    server_draft_id: &str,
    server_version: i64,
) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE local_drafts
         SET server_draft_id = $2, server_version = $3, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE draft_id = $1",
    )
    .bind(draft_id)
    .bind(server_draft_id)
    .bind(server_version)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE local_draft_operations
         SET sync_status = 'synced', last_error = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE local_operation_id = $1",
    )
    .bind(local_operation_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn mark_batch_operation_synced(
    pool: &SqlitePool,
    draft_id: &str,
    local_operation_id: &str,
    server_version: i64,
) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE local_drafts
         SET server_version = $2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE draft_id = $1",
    )
    .bind(draft_id)
    .bind(server_version)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE local_draft_operations
         SET sync_status = 'synced', last_error = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE local_operation_id = $1",
    )
    .bind(local_operation_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) fn map_daemon_operation_to_server(
    scope: DaemonDraftScope,
    _resource: DaemonDraftResourceKind,
    operation: &DaemonDraftOperation,
) -> Result<Option<ServerDraftOperationInput>, DaemonError> {
    if let Some(create) = &operation.create {
        return Ok(Some(ServerDraftOperationInput {
            action: ServerDraftOperationAction::Create,
            resource: ServerDraftResourceRef {
                scope,
                id: None,
                path: Some(create.path.clone()),
            },
            content: Some(create.content.clone()),
            new_path: None,
        }));
    }
    if let Some(update) = &operation.update {
        let update = match update {
            DaemonUpdateDraftOperation::Content(update) => update,
            DaemonUpdateDraftOperation::Text(_) => {
                return Err(DaemonError::InvalidRequest(
                    "text replacement update was not materialized before synchronization"
                        .to_owned(),
                ));
            }
        };
        return Ok(Some(ServerDraftOperationInput {
            action: ServerDraftOperationAction::Update,
            resource: ServerDraftResourceRef {
                scope,
                id: Some(update.id.clone()),
                path: None,
            },
            content: Some(update.content.clone()),
            new_path: None,
        }));
    }
    if let Some(rename) = &operation.rename {
        return Ok(Some(ServerDraftOperationInput {
            action: ServerDraftOperationAction::Rename,
            resource: ServerDraftResourceRef {
                scope,
                id: Some(rename.id.clone()),
                path: None,
            },
            content: None,
            new_path: Some(rename.new_path.clone()),
        }));
    }
    if let Some(delete) = &operation.delete {
        return Ok(Some(ServerDraftOperationInput {
            action: ServerDraftOperationAction::Delete,
            resource: ServerDraftResourceRef {
                scope,
                id: Some(delete.id.clone()),
                path: None,
            },
            content: None,
            new_path: None,
        }));
    }
    if operation.discard.is_some() {
        return Ok(None);
    }
    Err(DaemonError::InvalidRequest(
        "draft operation must contain exactly one operation variant".to_owned(),
    ))
}

pub(crate) fn draft_resource_kind_from_str(
    value: &str,
) -> Result<DaemonDraftResourceKind, DaemonError> {
    match value {
        // Legacy kind values from archived local databases are treated as
        // the unified Memory kind; the runtime only writes 'memory'.
        "context" | "rule" | "workflow" | "memory" => Ok(DaemonDraftResourceKind::Memory),
        other => Err(DaemonError::InvalidRequest(format!(
            "unknown draft resource kind: {other}"
        ))),
    }
}

pub(crate) fn daemon_draft_scope_from_str(value: &str) -> Result<DaemonDraftScope, DaemonError> {
    match value {
        "org" => Ok(DaemonDraftScope::Org),
        "project" => Ok(DaemonDraftScope::Project),
        other => Err(DaemonError::InvalidRequest(format!(
            "unknown draft scope: {other}"
        ))),
    }
}

pub(crate) fn local_draft_status_from_str(
    value: &str,
) -> Result<DaemonLocalDraftStatus, DaemonError> {
    match value {
        "open" => Ok(DaemonLocalDraftStatus::Open),
        "submitted" => Ok(DaemonLocalDraftStatus::Submitted),
        "discarded" => Ok(DaemonLocalDraftStatus::Discarded),
        "merged" => Ok(DaemonLocalDraftStatus::Merged),
        other => Err(DaemonError::InvalidRequest(format!(
            "unknown local draft status: {other}"
        ))),
    }
}

pub(crate) fn draft_freshness_from_str(value: &str) -> Result<DaemonDraftFreshness, DaemonError> {
    match value {
        "current" => Ok(DaemonDraftFreshness::Current),
        "behind" => Ok(DaemonDraftFreshness::Behind),
        other => Err(DaemonError::InvalidRequest(format!(
            "unknown draft freshness: {other}"
        ))),
    }
}

pub(crate) fn draft_reconciliation_status_from_str(
    value: &str,
) -> Result<DaemonDraftReconciliationStatus, DaemonError> {
    match value {
        "unknown" => Ok(DaemonDraftReconciliationStatus::Unknown),
        "clean" => Ok(DaemonDraftReconciliationStatus::Clean),
        "conflicts" => Ok(DaemonDraftReconciliationStatus::Conflicts),
        other => Err(DaemonError::InvalidRequest(format!(
            "unknown draft reconciliation status: {other}"
        ))),
    }
}

pub(crate) fn draft_operation_source_from_str(
    value: &str,
) -> Result<DaemonDraftOperationRecordSource, DaemonError> {
    match value {
        "desktop" => Ok(DaemonDraftOperationRecordSource::Desktop),
        "cli" => Ok(DaemonDraftOperationRecordSource::Cli),
        "mcp_store" => Ok(DaemonDraftOperationRecordSource::McpStore),
        "server" => Ok(DaemonDraftOperationRecordSource::Server),
        other => Err(DaemonError::InvalidRequest(format!(
            "unknown draft operation source: {other}"
        ))),
    }
}

pub(crate) fn draft_operation_sync_status_from_str(
    value: &str,
) -> Result<DraftOperationSyncStatus, DaemonError> {
    match value {
        "queued" => Ok(DraftOperationSyncStatus::Queued),
        "syncing" => Ok(DraftOperationSyncStatus::Syncing),
        "retrying" => Ok(DraftOperationSyncStatus::Retrying),
        "synced" => Ok(DraftOperationSyncStatus::Synced),
        "failed" => Ok(DraftOperationSyncStatus::Failed),
        other => Err(DaemonError::InvalidRequest(format!(
            "unknown draft operation sync status: {other}"
        ))),
    }
}

pub(crate) fn draft_title(operation: &QueuedDraftOperation) -> String {
    operation
        .path
        .as_deref()
        .or(operation.target_id.as_deref())
        .map(|target| format!("Draft for {target}"))
        .unwrap_or_else(|| "Draft operation".to_owned())
}

pub(crate) async fn pull_draft_events(state: &DaemonState) -> Result<(), DaemonError> {
    let access = crate::project_access::require_current(state)?;
    let mut cursor = load_meta_value(&state.inner.pool, META_DRAFT_EVENTS_CURSOR).await?;

    loop {
        let path = cursor
            .as_deref()
            .map(|cursor| format!("/api/v1/draft-events?after_cursor={cursor}"))
            .unwrap_or_else(|| "/api/v1/draft-events".to_owned());
        let response: ServerDraftEventListResponse = get_server_json(state, &path).await?;
        if response.events.is_empty() {
            if response.has_more {
                return Err(DaemonError::Server(
                    "Server returned an empty draft event page with has_more=true".to_owned(),
                ));
            }
            return Ok(());
        }

        let next_cursor = response.next_cursor.as_deref().ok_or_else(|| {
            DaemonError::Server("Server returned draft events without a next cursor".to_owned())
        })?;
        if cursor.as_deref() == Some(next_cursor) {
            return Err(DaemonError::Server(
                "Server draft event cursor did not advance".to_owned(),
            ));
        }

        // The Server projection is canonical even for events produced by this installation.
        // Re-projecting them refreshes coordination fields that an upload response cannot carry.
        let remote_events = response
            .events
            .iter()
            .filter(|event| access.project_ids.contains(&event.project_id))
            .collect::<Vec<_>>();
        let affected_projects = remote_events
            .iter()
            .map(|event| event.project_id.clone())
            .collect::<BTreeSet<_>>();
        let mut projection_requests = BTreeMap::new();
        for event in &remote_events {
            let request = projection_requests
                .entry(event.draft_id.clone())
                .or_insert_with(|| (event.project_id.clone(), event.version));
            if request.0 != event.project_id {
                return Err(DaemonError::Server(format!(
                    "Server returned draft {} events for multiple projects",
                    event.draft_id
                )));
            }
            request.1 = request.1.max(event.version);
        }

        let projection_gate = std::sync::Arc::new(tokio::sync::Semaphore::new(
            MAX_CONCURRENT_DRAFT_PROJECTION_REQUESTS,
        ));
        let mut projection_tasks = tokio::task::JoinSet::new();
        for (draft_id, (project_id, minimum_version)) in projection_requests {
            let state = state.clone();
            let projection_gate = projection_gate.clone();
            projection_tasks.spawn(async move {
                let _permit = projection_gate.acquire_owned().await.map_err(|_| {
                    DaemonError::Server("Draft projection request gate closed".to_owned())
                })?;
                let detail: ServerDraftProjectionDetail =
                    get_server_json(&state, &format!("/api/v1/drafts/{draft_id}")).await?;
                Ok::<_, DaemonError>((draft_id, project_id, minimum_version, detail))
            });
        }

        let mut drafts = BTreeMap::new();
        while let Some(result) = projection_tasks.join_next().await {
            let (draft_id, project_id, minimum_version, detail) = result.map_err(|error| {
                DaemonError::Server(format!("Draft projection request task failed: {error}"))
            })??;
            if detail.draft.draft_id != draft_id
                || detail.draft.project_id != project_id
                || detail.draft.version < minimum_version
            {
                return Err(DaemonError::Server(format!(
                    "Server returned an inconsistent projection for draft {}",
                    draft_id
                )));
            }
            drafts.insert(draft_id, detail);
        }

        let active_base_commit_ids = drafts
            .values()
            .filter(|detail| {
                matches!(
                    detail.draft.status,
                    DaemonLocalDraftStatus::Open | DaemonLocalDraftStatus::Submitted
                )
            })
            .filter_map(|detail| detail.draft.base_commit_id.clone())
            .collect::<BTreeSet<_>>();
        for base_commit_id in active_base_commit_ids {
            commit_sync::ensure_commit_cached(state, &base_commit_id).await?;
        }

        crate::project_access::require_current(state)?;
        let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
        for event in response
            .events
            .iter()
            .filter(|event| !access.project_ids.contains(&event.project_id))
        {
            sqlx::query(
                "INSERT INTO daemon_meta(key, value) VALUES ($1, '1') ON CONFLICT(key) DO NOTHING",
            )
            .bind(format!("{DEFERRED_EVENTS_PREFIX}{}", event.project_id))
            .execute(&mut *tx)
            .await?;
        }
        for detail in drafts.values() {
            project_server_draft(&mut tx, detail).await?;
        }
        for event in remote_events {
            sqlx::query(
                "INSERT INTO remote_draft_events (
                    event_id, draft_id, project_id, event_type, version, daemon_installation_id, created_at
                 )
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT(event_id) DO NOTHING",
            )
            .bind(&event.event_id)
            .bind(&event.draft_id)
            .bind(&event.project_id)
            .bind(&event.event_type)
            .bind(event.version)
            .bind(&event.daemon_installation_id)
            .bind(&event.created_at)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO daemon_meta (key, value)
             VALUES ($1, $2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(META_DRAFT_EVENTS_CURSOR)
        .bind(next_cursor)
        .execute(&mut *tx)
        .await?;
        for project_id in affected_projects {
            crate::search::scheduler::enqueue_project_in_tx(&mut tx, &project_id).await?;
        }
        tx.commit().await?;
        state.inner.search_index_notify.notify_one();

        cursor = response.next_cursor;
        if !response.has_more {
            return Ok(());
        }
    }
}

pub(crate) async fn project_server_draft(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    detail: &ServerDraftProjectionDetail,
) -> Result<(), DaemonError> {
    let existing = sqlx::query(
        "SELECT draft_id, server_version
         FROM local_drafts
         WHERE server_draft_id = $1",
    )
    .bind(&detail.draft.draft_id)
    .fetch_optional(&mut **tx)
    .await?;
    let local_draft_id = if let Some(row) = existing {
        let local_draft_id: String = row.try_get("draft_id")?;
        let server_version: i64 = row.try_get("server_version")?;
        if detail.draft.version < server_version {
            return Err(DaemonError::Server(format!(
                "Server draft {} regressed from version {server_version} to {}",
                detail.draft.draft_id, detail.draft.version
            )));
        }
        sqlx::query(
            "UPDATE local_drafts
             SET project_id = $2, server_version = $3, base_commit_id = $4,
                 current_commit_id = $5, freshness = $6,
                 has_upstream_resource_changes = $7, reconciliation = $8,
                 reconciliation_candidate_id = $9,
                 resource_scope = $10, resource_kind = $11, target_id = $12, path = $13,
                 status = $14, updated_at = $15
             WHERE draft_id = $1",
        )
        .bind(&local_draft_id)
        .bind(&detail.draft.project_id)
        .bind(detail.draft.version)
        .bind(&detail.draft.base_commit_id)
        .bind(&detail.draft.coordination.current_commit_id)
        .bind(detail.draft.coordination.freshness.as_str())
        .bind(detail.draft.coordination.has_upstream_resource_changes)
        .bind(detail.draft.coordination.reconciliation.as_str())
        .bind(&detail.draft.coordination.candidate_id)
        .bind(detail.draft.resource.scope.as_str())
        .bind(DaemonDraftResourceKind::Memory.as_str())
        .bind(&detail.draft.resource.id)
        .bind(&detail.draft.resource.path)
        .bind(detail.draft.status.as_str())
        .bind(&detail.draft.updated_at)
        .execute(&mut **tx)
        .await?;
        local_draft_id
    } else {
        sqlx::query(
            "INSERT INTO local_drafts (
                draft_id, project_id, server_draft_id, server_version, base_commit_id,
                current_commit_id, freshness, has_upstream_resource_changes,
                reconciliation, reconciliation_candidate_id,
                resource_scope, resource_kind, target_id, path,
                status, created_at, updated_at
             )
             VALUES ($1, $2, $1, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(&detail.draft.draft_id)
        .bind(&detail.draft.project_id)
        .bind(detail.draft.version)
        .bind(&detail.draft.base_commit_id)
        .bind(&detail.draft.coordination.current_commit_id)
        .bind(detail.draft.coordination.freshness.as_str())
        .bind(detail.draft.coordination.has_upstream_resource_changes)
        .bind(detail.draft.coordination.reconciliation.as_str())
        .bind(&detail.draft.coordination.candidate_id)
        .bind(detail.draft.resource.scope.as_str())
        .bind(DaemonDraftResourceKind::Memory.as_str())
        .bind(&detail.draft.resource.id)
        .bind(&detail.draft.resource.path)
        .bind(detail.draft.status.as_str())
        .bind(&detail.draft.created_at)
        .bind(&detail.draft.updated_at)
        .execute(&mut **tx)
        .await?;
        detail.draft.draft_id.clone()
    };

    let linked_rows = sqlx::query(
        "SELECT local_operation_id, server_operation_id
         FROM local_draft_operations
         WHERE draft_id = $1 AND server_operation_id IS NOT NULL",
    )
    .bind(&local_draft_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in linked_rows {
        let server_operation_id: String = row.try_get("server_operation_id")?;
        if detail
            .operations
            .iter()
            .any(|operation| operation.operation_id == server_operation_id)
        {
            continue;
        }
        sqlx::query("DELETE FROM local_draft_operations WHERE local_operation_id = $1")
            .bind(row.try_get::<String, _>("local_operation_id")?)
            .execute(&mut **tx)
            .await?;
    }

    let rows = sqlx::query(
        "SELECT local_operation_id, operation_json
         FROM local_draft_operations
         WHERE draft_id = $1
           AND server_operation_id IS NULL
           AND source != 'server'
         ORDER BY rowid",
    )
    .bind(&local_draft_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut unlinked_operations = rows
        .iter()
        .map(|row| {
            Ok(UnlinkedLocalOperation {
                local_operation_id: row.try_get("local_operation_id")?,
                operation: serde_json::from_str(&row.try_get::<String, _>("operation_json")?)?,
                linked: false,
            })
        })
        .collect::<Result<Vec<_>, DaemonError>>()?;

    for server_operation in &detail.operations {
        if server_operation.resource.scope != detail.draft.resource.scope {
            return Err(DaemonError::Server(format!(
                "Server operation {} does not match draft {} resource",
                server_operation.operation_id, detail.draft.draft_id
            )));
        }
        let operation = map_server_operation_to_daemon(server_operation)?;
        let already_linked: Option<String> = sqlx::query_scalar(
            "SELECT local_operation_id
             FROM local_draft_operations
             WHERE server_operation_id = $1",
        )
        .bind(&server_operation.operation_id)
        .fetch_optional(&mut **tx)
        .await?;
        if already_linked.is_some() {
            continue;
        }

        if let Some(local_operation) = unlinked_operations.iter_mut().find(|candidate| {
            !candidate.linked && draft_operations_match(&candidate.operation, &operation)
        }) {
            sqlx::query(
                "UPDATE local_draft_operations
                 SET server_operation_id = $2,
                     sync_status = 'synced',
                     last_error = NULL,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE local_operation_id = $1",
            )
            .bind(&local_operation.local_operation_id)
            .bind(&server_operation.operation_id)
            .execute(&mut **tx)
            .await?;
            local_operation.linked = true;
            continue;
        }

        sqlx::query(
            "INSERT INTO local_draft_operations (
                local_operation_id, draft_id, server_operation_id, resource_kind,
                operation_json, source, sync_status, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, 'server', 'synced', $6, $6)",
        )
        .bind(format!("server_{}", server_operation.operation_id))
        .bind(&local_draft_id)
        .bind(&server_operation.operation_id)
        .bind(DaemonDraftResourceKind::Memory.as_str())
        .bind(serde_json::to_string(&operation)?)
        .bind(&server_operation.created_at)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub(crate) fn map_server_operation_to_daemon(
    operation: &ServerDraftProjectionOperation,
) -> Result<DaemonDraftOperation, DaemonError> {
    let missing = |field: &str| {
        DaemonError::Server(format!(
            "Server operation {} is missing {field}",
            operation.operation_id
        ))
    };
    let mut mapped = DaemonDraftOperation {
        create: None,
        update: None,
        rename: None,
        delete: None,
        discard: None,
    };
    match operation.action {
        ServerDraftOperationAction::Create => {
            mapped.create = Some(DaemonCreateDraftOperation {
                path: operation
                    .resource
                    .path
                    .clone()
                    .ok_or_else(|| missing("path"))?,
                content: operation
                    .content
                    .clone()
                    .ok_or_else(|| missing("content"))?,
                description: None,
            });
        }
        ServerDraftOperationAction::Update => {
            mapped.update = Some(DaemonUpdateDraftOperation::Content(
                DaemonContentDraftUpdate {
                    id: operation.resource.id.clone().ok_or_else(|| missing("id"))?,
                    content: operation
                        .content
                        .clone()
                        .ok_or_else(|| missing("content"))?,
                    description: None,
                },
            ));
        }
        ServerDraftOperationAction::Rename => {
            mapped.rename = Some(DaemonRenameDraftOperation {
                id: operation.resource.id.clone().ok_or_else(|| missing("id"))?,
                new_path: operation
                    .new_path
                    .clone()
                    .ok_or_else(|| missing("new_path"))?,
                description: None,
            });
        }
        ServerDraftOperationAction::Delete => {
            mapped.delete = Some(DaemonDeleteDraftOperation {
                id: operation.resource.id.clone().ok_or_else(|| missing("id"))?,
                description: None,
            });
        }
    }
    Ok(mapped)
}

pub(crate) fn draft_operations_match(
    left: &DaemonDraftOperation,
    right: &DaemonDraftOperation,
) -> bool {
    match (left, right) {
        (
            DaemonDraftOperation {
                create: Some(left), ..
            },
            DaemonDraftOperation {
                create: Some(right),
                ..
            },
        ) => left.path == right.path && left.content == right.content,
        (
            DaemonDraftOperation {
                update: Some(left), ..
            },
            DaemonDraftOperation {
                update: Some(right),
                ..
            },
        ) => match (left, right) {
            (
                DaemonUpdateDraftOperation::Content(left),
                DaemonUpdateDraftOperation::Content(right),
            ) => left.id == right.id && left.content == right.content,
            _ => false,
        },
        (
            DaemonDraftOperation {
                rename: Some(left), ..
            },
            DaemonDraftOperation {
                rename: Some(right),
                ..
            },
        ) => left.id == right.id && left.new_path == right.new_path,
        (
            DaemonDraftOperation {
                delete: Some(left), ..
            },
            DaemonDraftOperation {
                delete: Some(right),
                ..
            },
        ) => left.id == right.id,
        _ => false,
    }
}

pub(crate) struct UnlinkedLocalOperation {
    local_operation_id: String,
    operation: DaemonDraftOperation,
    linked: bool,
}
