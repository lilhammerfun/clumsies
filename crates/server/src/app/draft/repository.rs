//! SQL queries and persistence for draft resources.

use super::model::{content_for_kind, draft_event_type, draft_operation_action, draft_status};
use crate::app::auth::AuthPrincipal;
use crate::app::draft::dto::{
    Draft, DraftCoordination, DraftEvent, DraftEventListResponse, DraftEventType, DraftFreshness,
    DraftListResponse, DraftOperation, DraftOperationInput, DraftReconciliationStatus,
    DraftResourceContent, DraftResourceRef, ReconciliationConflict, ReconciliationResourceState,
};
use crate::app::memory::model::resource_scope;
use crate::app::organization::dto::UserRef;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use crate::pagination::page_info;
use sqlx::types::Json;
use sqlx::{FromRow, PgPool, Postgres, Row, Transaction};

/// Load public identity fields for a required user within the caller's transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
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
    UserRef::from_row(&row).map_err(ServerError::from)
}

/// Resolve an explicit identity or path against the ancestor, current resources, and optional
/// history.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates query failures and rejects a historical path associated with multiple resource
/// identities.
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

/// Require the target to be active organization Memory selected by the carrying project.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
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

/// Lock the proposal and allocate its next dense operation ordinal before inserting the mutation.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates missing-proposal, serialization, and database failures. The acquired row lock
/// remains held until the caller ends the transaction.
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

/// Mark all current reconciliation evidence for the proposal as stale.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
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

/// Persist a proposal lifecycle event and return its monotonic synchronization cursor.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
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

/// Read proposal mutations in their persisted semantic order.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
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

/// Read one resource's identity and content at the specified ancestor snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource
/// and rejects inconsistent persisted state or resource selections.
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

/// Check whether a different active resource occupies the proposed path in its ownership scope.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
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

/// Check proposal authorship together with organization and project membership.
///
/// # Errors
/// Propagates database access and row-decoding failures.
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

/// Require every proposal in a batch to belong to the accessible authenticated author.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
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

/// Require all supplied proposal identities to have the expected author.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects identities outside the
/// required ownership boundary.
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

/// Read proposals for the supplied author and optional carrying project.
///
/// # Errors
/// Propagates database access and row-decoding failures.
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
            EXISTS (
                SELECT 1 FROM draft_rebases r
                JOIN draft_reconciliation_candidates c ON c.candidate_id = r.candidate_id
                WHERE r.draft_id = d.draft_id AND r.resulting_draft_version = d.version
                  AND c.status = 'clean'
                  AND d.base_commit_id IS NOT DISTINCT FROM current_ref.commit_id
            ) AS auto_rebased,
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
                author: UserRef::from_row(row)?,
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

/// Read the author's lifecycle events after a validated exclusive cursor with one look-ahead row.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
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

/// Decode a stored lifecycle event and its client synchronization correlation.
///
/// # Errors
/// Propagates database access and row-decoding failures.
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

/// Decode upstream freshness and candidate availability from the proposal projection.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
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
        auto_rebased: row.try_get("auto_rebased")?,
    })
}

/// Read proposal scope and carrying project before acquiring coordination locks.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn load_identity(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftIdentity>, ServerError> {
    Ok(sqlx::query_as::<_, DraftIdentity>(
        "SELECT project_id, resource_scope
         FROM drafts
         WHERE draft_id = $1",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Lock proposal revision, lifecycle, and resource identity before appending a mutation.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_append_state(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftAppendState>, ServerError> {
    Ok(sqlx::query_as::<_, DraftAppendState>(
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
    .await?)
}

/// Advance a proposal revision and return its new value within the caller's transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn increment_version(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<DraftEventState, ServerError> {
    Ok(sqlx::query_as::<_, DraftEventState>(
        "UPDATE drafts
         SET version = version + 1, updated_at = now()
         WHERE draft_id = $1
         RETURNING project_id, version",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Read proposal metadata and its public author identity for detail assembly.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn load_detail_record(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftDetailRecord>, ServerError> {
    Ok(sqlx::query_as::<_, DraftDetailRecord>(
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
    .await?)
}

/// Find live reconciliation evidence matching the exact proposal and upstream revisions.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn find_candidate_summary(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    draft_version: i64,
    base_commit_id: &Option<String>,
    current_commit_id: &Option<String>,
) -> Result<Option<CandidateSummary>, ServerError> {
    Ok(sqlx::query_as::<_, CandidateSummary>(
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
    .bind(base_commit_id)
    .bind(current_commit_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Read proposal scope, resource identity, and ancestor needed to replay its mutations.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn load_base_state(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftBaseState>, ServerError> {
    Ok(sqlx::query_as::<_, DraftBaseState>(
        "SELECT base_commit_id, resource_scope, resource_kind, target_id, path
         FROM drafts WHERE draft_id = $1",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Lock proposal state before computing and persisting a reconciliation candidate.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_reconciliation_state(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftReconciliationState>, ServerError> {
    Ok(sqlx::query_as::<_, DraftReconciliationState>(
        "SELECT project_id, base_commit_id, resource_scope, resource_kind,
                target_id, path, status, version
         FROM drafts
         WHERE draft_id = $1
         FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Locate uninvalidated evidence for the exact proposal and upstream result fingerprints.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn find_current_candidate(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    draft_version: i64,
    base_commit_id: &Option<String>,
    current_commit_id: &Option<String>,
) -> Result<Option<String>, ServerError> {
    Ok(sqlx::query_scalar::<_, String>(
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
    .bind(base_commit_id)
    .bind(current_commit_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Persist reconciliation inputs, result, and conflicts for later revision-checked application.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn insert_candidate(
    tx: &mut Transaction<'_, Postgres>,
    input: NewCandidate<'_>,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO draft_reconciliation_candidates (
            candidate_id, draft_id, draft_version, base_commit_id, current_commit_id,
            status, base_state, current_state, draft_state, proposed_state,
            conflicts, result_hash
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(input.candidate_id)
    .bind(input.draft_id)
    .bind(input.draft_version)
    .bind(input.base_commit_id)
    .bind(input.current_commit_id)
    .bind(input.status)
    .bind(Json(input.base_state))
    .bind(Json(input.current_state))
    .bind(Json(input.draft_state))
    .bind(input.proposed_state.map(Json))
    .bind(Json(input.conflicts))
    .bind(input.result_hash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Lock proposal lifecycle and revision before replacing its ancestor and mutations.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_rebase_state(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftRebaseState>, ServerError> {
    Ok(sqlx::query_as::<_, DraftRebaseState>(
        "SELECT project_id, author_user_id, base_commit_id, resource_scope,
                resource_kind, target_id, path, status, version, title, description
         FROM drafts WHERE draft_id = $1 FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Archive the proposal's pre-rebase metadata and ordered mutations as one immutable revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn insert_revision(
    tx: &mut Transaction<'_, Postgres>,
    input: NewDraftRevision<'_>,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO draft_revisions (
            revision_id, draft_id, draft_version, base_commit_id, lifecycle_status,
            title, description, operations
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(input.revision_id)
    .bind(input.draft_id)
    .bind(input.draft_version)
    .bind(input.base_commit_id)
    .bind(input.lifecycle_status)
    .bind(input.title)
    .bind(input.description)
    .bind(Json(input.operations))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Remove the proposal's old mutation sequence before inserting its replacement.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn delete_operations(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<(), ServerError> {
    sqlx::query("DELETE FROM draft_operations WHERE draft_id = $1")
        .bind(draft_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Replace a proposal's ancestor and advance the caller-supplied revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn advance_base(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    current_ref: &Option<String>,
) -> Result<i64, ServerError> {
    Ok(sqlx::query_scalar(
        "UPDATE drafts
         SET base_commit_id = $2, version = version + 1, updated_at = now()
         WHERE draft_id = $1 RETURNING version",
    )
    .bind(draft_id)
    .bind(current_ref)
    .fetch_one(&mut **tx)
    .await?)
}

/// Lock an approved review's content fingerprint before a proposal rebase.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_review_approval(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<ReviewApprovalState>, ServerError> {
    Ok(sqlx::query_as::<_, ReviewApprovalState>(
        "SELECT reviews.review_id, reviews.status, reviews.approved_result_hash
         FROM reviews
         JOIN review_drafts ON review_drafts.review_id = reviews.review_id
         WHERE review_drafts.draft_id = $1
         FOR UPDATE OF reviews",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Persist the approval state chosen after comparing old and rebased proposal content.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn update_review_approval(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    preserve_approval: bool,
) -> Result<(), ServerError> {
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
        .bind(review_id)
        .bind(preserve_approval)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Record the candidate and saved revision used to advance the proposal's ancestor.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn insert_rebase(
    tx: &mut Transaction<'_, Postgres>,
    input: NewDraftRebase<'_>,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO draft_rebases (
            rebase_id, draft_id, candidate_id, previous_revision_id,
            applied_by_user_id, resulting_draft_version, result_hash
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(input.rebase_id)
    .bind(input.draft_id)
    .bind(input.candidate_id)
    .bind(input.previous_revision_id)
    .bind(input.author_user_id)
    .bind(input.next_version)
    .bind(input.result_hash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Find the review associated with a proposal, if one exists.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn find_review(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<String>, ServerError> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT review_id FROM review_drafts WHERE draft_id = $1")
            .bind(draft_id)
            .fetch_optional(&mut **tx)
            .await?,
    )
}

/// Persist initial proposal identity, ownership, lifecycle, and ancestor metadata.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn insert_draft(
    tx: &mut Transaction<'_, Postgres>,
    input: NewDraft<'_>,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO drafts (
                draft_id, project_id, author_user_id, title, description,
                resource_scope, resource_kind, base_commit_id, target_id, path, status, version,
                daemon_installation_id
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'open', 1, $11)",
    )
    .bind(input.draft_id)
    .bind(input.project_id)
    .bind(input.author_user_id)
    .bind(input.title)
    .bind(input.description)
    .bind(input.scope)
    .bind(input.resource_kind)
    .bind(input.base_commit_id)
    .bind(input.target_id)
    .bind(input.path)
    .bind(input.daemon_installation_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Lock proposal title, description, lifecycle, and revision before a metadata edit.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_metadata(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftMetadataState>, ServerError> {
    Ok(sqlx::query_as::<_, DraftMetadataState>(
        "SELECT title, description, status, version, project_id
             FROM drafts
             WHERE draft_id = $1
             FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Persist replacement proposal metadata and advance its concurrency revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn update_metadata(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    title: String,
    description: String,
) -> Result<DraftEventState, ServerError> {
    Ok(sqlx::query_as::<_, DraftEventState>(
        "UPDATE drafts
             SET title = $2, description = $3, version = version + 1, updated_at = now()
             WHERE draft_id = $1
             RETURNING project_id, version",
    )
    .bind(draft_id)
    .bind(title)
    .bind(description)
    .fetch_one(&mut **tx)
    .await?)
}

/// Lock proposal lifecycle and revision before discarding it.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_discard_state(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftDiscardState>, ServerError> {
    Ok(sqlx::query_as::<_, DraftDiscardState>(
        "SELECT project_id, status, version FROM drafts WHERE draft_id = $1 FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Persist a proposal's discarded lifecycle and increment its revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn mark_discarded(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<i64, ServerError> {
    Ok(sqlx::query_scalar(
        "UPDATE drafts SET status = 'discarded', version = version + 1, updated_at = now()
             WHERE draft_id = $1 RETURNING version",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Read the owning organizations of a batch's proposals for ordered coordination locking.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn list_batch_organizations(
    tx: &mut Transaction<'_, Postgres>,
    draft_ids: &Vec<String>,
) -> Result<Vec<DraftBatchOrganization>, ServerError> {
    Ok(sqlx::query_as::<_, DraftBatchOrganization>(
        "SELECT DISTINCT project.org_id
             FROM drafts AS draft
             JOIN projects AS project ON project.project_id = draft.project_id
             WHERE draft.draft_id = ANY($1)
               AND draft.resource_scope = 'org'
             ORDER BY project.org_id",
    )
    .bind(draft_ids)
    .fetch_all(&mut **tx)
    .await?)
}

/// Read persisted reconciliation evidence, including its inputs and invalidation state.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn load_candidate_record(
    tx: &mut Transaction<'_, Postgres>,
    candidate_id: &str,
    draft_id: &str,
) -> Result<Option<CandidateRecord>, ServerError> {
    Ok(sqlx::query_as::<_, CandidateRecord>(
        "SELECT candidate_id, draft_id, draft_version, base_commit_id, current_commit_id,
                status, base_state, current_state, draft_state, proposed_state,
                conflicts, result_hash, created_at, invalidated_at
         FROM draft_reconciliation_candidates
         WHERE candidate_id = $1 AND draft_id = $2",
    )
    .bind(candidate_id)
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Read the current proposal revision and lifecycle used to validate stored evidence.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn load_candidate_draft_state(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<CandidateDraftState, ServerError> {
    Ok(sqlx::query_as::<_, CandidateDraftState>(
        "SELECT project_id, base_commit_id, resource_scope, version
         FROM drafts WHERE draft_id = $1",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Invalidate one stale reconciliation candidate within the caller's transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn invalidate_candidate(
    tx: &mut Transaction<'_, Postgres>,
    candidate_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE draft_reconciliation_candidates SET invalidated_at = now()
             WHERE candidate_id = $1 AND invalidated_at IS NULL",
    )
    .bind(candidate_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Stored reconciliation inputs, result, conflicts, and invalidation timestamp.
#[derive(sqlx::FromRow)]
pub(super) struct CandidateRecord {
    /// Identifier of the reconciliation result being inspected or applied.
    pub(super) candidate_id: String,
    /// Stable identifier of the editable proposal.
    pub(super) draft_id: String,
    /// Proposal revision to which this record or candidate applies.
    pub(super) draft_version: i64,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Reference head observed when computing freshness or reconciliation.
    pub(super) current_commit_id: Option<String>,
    /// Stored reconciliation outcome before checking whether the evidence is still valid.
    pub(super) status: String,
    /// Resource state at the proposal's ancestor commit.
    pub(super) base_state: Json<ReconciliationResourceState>,
    /// Resource state at the upstream reference head.
    pub(super) current_state: Json<ReconciliationResourceState>,
    /// Resource state produced by applying the proposal to its ancestor.
    pub(super) draft_state: Json<ReconciliationResourceState>,
    /// Automatically reconciled result, absent when conflicts require user resolution.
    pub(super) proposed_state: Option<Json<ReconciliationResourceState>>,
    /// Unresolved differences between ancestor, upstream, and proposed content.
    pub(super) conflicts: Json<Vec<ReconciliationConflict>>,
    /// Fingerprint of materialized content used to validate reconciliation or approval.
    pub(super) result_hash: Option<String>,
    /// UTC timestamp at which the record was created.
    pub(super) created_at: time::OffsetDateTime,
    /// UTC time at which a reconciliation candidate ceased to be usable.
    pub(super) invalidated_at: Option<time::OffsetDateTime>,
}

/// Current proposal revision and lifecycle used to reject stale reconciliation evidence.
#[derive(sqlx::FromRow)]
pub(super) struct CandidateDraftState {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
}

/// Carrying project and ownership scope used before coordination locking.
#[derive(sqlx::FromRow)]
pub(super) struct DraftIdentity {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
}

/// Locked proposal metadata needed to validate an appended mutation.
#[derive(sqlx::FromRow)]
pub(super) struct DraftAppendState {
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Whether the first draft operation introduces a new resource identity.
    pub(super) creates_resource: bool,
}

/// Project identity and new proposal revision needed to persist a lifecycle event.
#[derive(sqlx::FromRow)]
pub(super) struct DraftEventState {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
}

/// Stored proposal metadata and public author identity used for detail assembly.
#[derive(sqlx::FromRow)]
pub(super) struct DraftDetailRecord {
    /// Stable identifier of the editable proposal.
    pub(super) draft_id: String,
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Human-readable summary of a proposal or review.
    pub(super) title: String,
    /// Human-readable explanation associated with the resource.
    pub(super) description: String,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
    /// Stable identity of the resource affected by the operation.
    pub(super) target_id: Option<String>,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub(super) path: Option<String>,
    /// Client installation identity used to correlate draft synchronization events.
    pub(super) daemon_installation_id: String,
    /// UTC timestamp at which the record was created.
    pub(super) created_at: time::OffsetDateTime,
    /// UTC timestamp of the latest persisted change.
    pub(super) updated_at: time::OffsetDateTime,
    /// Public identity of the author of this record.
    #[sqlx(flatten)]
    pub(super) author: crate::app::organization::dto::UserRef,
}

/// Identity and conflict status of evidence matching an exact proposal and upstream revision.
#[derive(sqlx::FromRow)]
pub(super) struct CandidateSummary {
    /// Identifier of the reconciliation result being inspected or applied.
    pub(super) candidate_id: String,
    /// Whether the matching reconciliation evidence contains conflicts.
    pub(super) status: String,
}

/// Resource identity and ancestor needed to replay a proposal's ordered mutations.
#[derive(sqlx::FromRow)]
pub(super) struct DraftBaseState {
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
    /// Stable identity of the resource affected by the operation.
    pub(super) target_id: Option<String>,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub(super) path: Option<String>,
}

/// Locked resource identity and proposal revision used to compute reconciliation evidence.
#[derive(sqlx::FromRow)]
pub(super) struct DraftReconciliationState {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
    /// Stable identity of the resource affected by the operation.
    pub(super) target_id: Option<String>,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub(super) path: Option<String>,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
}

/// Locked proposal lifecycle and revision before replacing its ancestor and mutations.
#[derive(sqlx::FromRow)]
pub(super) struct DraftRebaseState {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Persisted identity of the author whose ownership is checked by the use case.
    pub(super) author_user_id: String,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
    /// Human-readable summary of a proposal or review.
    pub(super) title: String,
    /// Human-readable explanation associated with the resource.
    pub(super) description: String,
}

/// Locked review approval and content fingerprint checked during a proposal rebase.
#[derive(sqlx::FromRow)]
pub(super) struct ReviewApprovalState {
    /// Stable identifier of a review spanning one or more proposals.
    pub(super) review_id: String,
    /// Review lifecycle whose approval may survive a content-preserving rebase.
    pub(super) status: String,
    /// Content fingerprint covered by the current review approval.
    pub(super) approved_result_hash: Option<String>,
}

/// Locked title, description, lifecycle, and revision needed for a metadata update.
#[derive(sqlx::FromRow)]
pub(super) struct DraftMetadataState {
    /// Human-readable summary of a proposal or review.
    pub(super) title: String,
    /// Human-readable explanation associated with the resource.
    pub(super) description: String,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
}

/// Locked lifecycle and revision used to validate a proposal discard.
#[derive(sqlx::FromRow)]
pub(super) struct DraftDiscardState {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
}

/// Proposal ownership used to acquire organization coordination locks in stable order.
#[derive(sqlx::FromRow)]
pub(super) struct DraftBatchOrganization {
    /// Organization boundary to which the resource or identity belongs.
    pub(super) org_id: String,
}

/// Borrowed reconciliation states and exact revisions to persist as reusable evidence.
pub(super) struct NewCandidate<'a> {
    /// Identifier of the reconciliation result being inspected or applied.
    pub(super) candidate_id: &'a str,
    /// Stable identifier of the editable proposal.
    pub(super) draft_id: &'a str,
    /// Proposal revision to which this record or candidate applies.
    pub(super) draft_version: i64,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: &'a Option<String>,
    /// Reference head observed when computing freshness or reconciliation.
    pub(super) current_commit_id: &'a Option<String>,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: &'a str,
    /// Resource state at the proposal's ancestor commit.
    pub(super) base_state: &'a ReconciliationResourceState,
    /// Resource state at the upstream reference head.
    pub(super) current_state: &'a ReconciliationResourceState,
    /// Resource state produced by applying the proposal to its ancestor.
    pub(super) draft_state: &'a ReconciliationResourceState,
    /// Automatically reconciled result, absent when conflicts require user resolution.
    pub(super) proposed_state: Option<&'a ReconciliationResourceState>,
    /// Unresolved differences between ancestor, upstream, and proposed content.
    pub(super) conflicts: &'a [ReconciliationConflict],
    /// Fingerprint of materialized content used to validate reconciliation or approval.
    pub(super) result_hash: &'a Option<String>,
}

/// Pre-rebase proposal metadata and ordered mutations saved as immutable history.
pub(super) struct NewDraftRevision<'a> {
    /// Identifier of an immutable saved draft revision.
    pub(super) revision_id: &'a str,
    /// Stable identifier of the editable proposal.
    pub(super) draft_id: &'a str,
    /// Proposal revision to which this record or candidate applies.
    pub(super) draft_version: i64,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Proposal lifecycle captured before replacing its ancestor and operations.
    pub(super) lifecycle_status: &'a str,
    /// Human-readable summary of a proposal or review.
    pub(super) title: String,
    /// Human-readable explanation associated with the resource.
    pub(super) description: String,
    /// Ordered mutations applied to the proposal's base state.
    pub(super) operations: &'a [DraftOperation],
}

/// Candidate, saved revision, and resulting proposal revision recorded for a rebase.
pub(super) struct NewDraftRebase<'a> {
    /// Stable identifier of the recorded reconciliation application.
    pub(super) rebase_id: &'a str,
    /// Stable identifier of the editable proposal.
    pub(super) draft_id: &'a str,
    /// Identifier of the reconciliation result being inspected or applied.
    pub(super) candidate_id: &'a str,
    /// Saved draft revision that permits auditing the state before rebase.
    pub(super) previous_revision_id: &'a str,
    /// Authorized author or administrator who applied this rebase.
    pub(super) author_user_id: &'a str,
    /// Proposal revision to publish after the current mutation.
    pub(super) next_version: i64,
    /// Fingerprint of materialized content used to validate reconciliation or approval.
    pub(super) result_hash: &'a str,
}

/// Validated identity, author, ancestor, and metadata for initial proposal persistence.
pub(super) struct NewDraft<'a> {
    /// Stable identifier of the editable proposal.
    pub(super) draft_id: &'a str,
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: &'a str,
    /// Persisted identity of the author whose ownership is checked by the use case.
    pub(super) author_user_id: &'a str,
    /// Human-readable summary of a proposal or review.
    pub(super) title: &'a str,
    /// Human-readable explanation associated with the resource.
    pub(super) description: &'a str,
    /// Ownership boundary determining which reference and resource set apply.
    pub(super) scope: &'a str,
    /// Stored content category used to validate and materialize the payload.
    pub(super) resource_kind: &'a str,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: &'a Option<String>,
    /// Stable identity of the resource affected by the operation.
    pub(super) target_id: &'a Option<String>,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub(super) path: &'a Option<String>,
    /// Client installation identity used to correlate draft synchronization events.
    pub(super) daemon_installation_id: &'a str,
}

/// Whether this exact proposal revision was persisted from a conflict-free candidate.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn was_auto_rebased(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    version: i64,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM draft_rebases r
            JOIN draft_reconciliation_candidates c ON c.candidate_id = r.candidate_id
            WHERE r.draft_id = $1 AND r.resulting_draft_version = $2 AND c.status = 'clean'
        )",
    )
    .bind(draft_id)
    .bind(version)
    .fetch_one(&mut **tx)
    .await?)
}
