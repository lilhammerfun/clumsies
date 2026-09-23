//! SQL queries and persistence for memory resources.

use super::dto::MemoryDetail;
use super::model::etag;
use super::model::{TargetResource, resource_status};
use crate::app::bundle::dto::MemoryExportBundle;
use crate::app::draft::dto::DraftResourceRef;
use crate::app::memory::dto::{
    MemoryExport, MemoryExportDraft, MemoryExportItem, MemoryExportSelection, MemoryMeta,
    ProjectOrgSelection,
};
use crate::app::memory::model::{content_hash, name_from_path, resource_scope};
use crate::app::organization::dto::UserRef;
use crate::error::ServerError;
use sqlx::{FromRow, PgPool, Postgres, Row, Transaction};
use std::collections::BTreeSet;

/// Read the selection revision used for optimistic concurrency checks.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn current_project_org_selection_revision(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<i64, ServerError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT revision
         FROM project_org_selection_states
         WHERE project_id = $1
         FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("project_org_selection", project_id))
}

/// Serializes transitions that can change the relationship between Project
/// selections and active Organization Drafts: replacing a selection,
/// creating/appending a Draft, and merging Organization authority. The lock
/// is Organization-scoped because one delete merge projects into every
/// selecting Project. It is acquired before Draft/ref/selection rows, works
/// across server instances, and therefore avoids opposing row-lock orders.
///
/// # Errors
/// Propagates database or lock-acquisition failures. The advisory lock is released with the
/// caller's transaction.
pub(crate) async fn lock_org_draft_selection_coordination(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(
            hashtextextended('org_draft_selection:' || $1, 0)
         )",
    )
    .bind(org_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Increment a project's selected-resource concurrency revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn update_project_org_selection_revision(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    revision: i64,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE project_org_selection_states
         SET revision = $2, updated_at = now()
         WHERE project_id = $1",
    )
    .bind(project_id)
    .bind(revision)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Persist a validated complete set of selected organization resource identities.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource
/// and rejects inconsistent persisted state or resource selections.
pub(crate) async fn insert_project_org_selection_items(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
    revision: i64,
    resource_ids: &[String],
) -> Result<(), ServerError> {
    let mut seen = BTreeSet::new();
    for resource_id in resource_ids {
        if !seen.insert(resource_id) {
            return Err(ServerError::InvalidRequest(format!(
                "project org selection contains duplicate resource: {resource_id}"
            )));
        }
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1
                FROM resources
                WHERE resource_id = $1
                  AND org_id = $2
                  AND scope = 'org'
                  AND status = 'active'
            )",
        )
        .bind(resource_id)
        .bind(org_id)
        .fetch_one(&mut **tx)
        .await?;
        if !exists {
            return Err(ServerError::not_found("org_resource", resource_id));
        }
        sqlx::query(
            "INSERT INTO project_org_resource_selections (
                project_id, resource_id, revision
             )
             VALUES ($1, $2, $3)",
        )
        .bind(project_id)
        .bind(resource_id)
        .bind(revision)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Read active resource metadata in deterministic path order for the requested scope.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
pub(crate) async fn list_resource_rows(
    pool: &PgPool,
    scope: &str,
    org_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<Vec<sqlx::postgres::PgRow>, ServerError> {
    let rows = if let Some(project_id) = project_id {
        sqlx::query(
            "SELECT
                resource_id, scope, project_id, path, name, description, status, org_source,
                content_hash, updated_at
             FROM resources
             WHERE scope = $1 AND project_id = $2 AND status = 'active'
             ORDER BY path",
        )
        .bind(scope)
        .bind(project_id)
        .fetch_all(pool)
        .await?
    } else if let Some(org_id) = org_id {
        sqlx::query(
            "SELECT
                resource_id, scope, project_id, path, name, description, status, org_source,
                content_hash, updated_at
             FROM resources
             WHERE scope = $1 AND org_id = $2 AND status = 'active'
             ORDER BY path",
        )
        .bind(scope)
        .bind(org_id)
        .fetch_all(pool)
        .await?
    } else {
        return Err(ServerError::InvalidRequest(
            "resource query requires org_id or project_id".to_owned(),
        ));
    };
    Ok(rows)
}

/// Read required resource content constrained to its organization or project scope.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource
/// and rejects inconsistent persisted state or resource selections.
pub(crate) async fn load_resource_detail_row(
    tx: &mut Transaction<'_, Postgres>,
    resource_id: &str,
    scope: &str,
    org_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<sqlx::postgres::PgRow, ServerError> {
    let row = if let Some(project_id) = project_id {
        sqlx::query(
            "SELECT
                resource_id, scope, project_id, path, name, description, status, org_source,
                revision, content_hash, body, updated_at
             FROM resources
             WHERE resource_id = $1
               AND scope = $2
               AND project_id = $3
               AND status = 'active'",
        )
        .bind(resource_id)
        .bind(scope)
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
    } else if let Some(org_id) = org_id {
        sqlx::query(
            "SELECT
                resource_id, scope, project_id, path, name, description, status, org_source,
                revision, content_hash, body, updated_at
             FROM resources
             WHERE resource_id = $1
               AND scope = $2
               AND org_id = $3
               AND status = 'active'",
        )
        .bind(resource_id)
        .bind(scope)
        .bind(org_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        return Err(ServerError::InvalidRequest(
            "resource detail requires org_id or project_id".to_owned(),
        ));
    };
    row.ok_or_else(|| ServerError::not_found("resource", resource_id))
}

/// Decode public Memory metadata and author identity from a resource row.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) fn memory_meta_from_row(row: &sqlx::postgres::PgRow) -> Result<MemoryMeta, ServerError> {
    Ok(MemoryMeta {
        org_source: row
            .try_get::<Option<sqlx::types::Json<crate::app::memory::dto::OrgMemorySource>>, _>(
                "org_source",
            )?
            .map(|source| source.0),
        memory_id: row.try_get("resource_id")?,
        scope: resource_scope(row.try_get::<String, _>("scope")?.as_str())?,
        project_id: row.try_get("project_id")?,
        path: row.try_get("path")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        content_hash: row.try_get("content_hash")?,
        status: resource_status(row.try_get::<String, _>("status")?.as_str())?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Assemble selected organization resource metadata and the project's selection revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_project_org_selection(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<ProjectOrgSelection, ServerError> {
    let revision = sqlx::query_scalar::<_, i64>(
        "SELECT revision
         FROM project_org_selection_states
         WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("project_org_selection", project_id))?;

    let rows = sqlx::query(
        "SELECT
            r.resource_id, r.scope, r.project_id, r.path, r.name, r.description, r.org_source,
            r.status, r.content_hash, r.updated_at
         FROM project_org_resource_selections s
         JOIN resources r ON r.resource_id = s.resource_id
         WHERE s.project_id = $1 AND r.status = 'active'
         ORDER BY r.path",
    )
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;

    let memories = rows
        .iter()
        .map(memory_meta_from_row)
        .collect::<Result<_, _>>()?;

    Ok(ProjectOrgSelection {
        project_id: project_id.to_owned(),
        memories,
        revision,
    })
}

/// Resolve a mutation target by stable identity or path within its required scope.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource
/// and rejects inconsistent persisted state or resource selections.
pub(crate) async fn load_target_resource(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    project_id: Option<&str>,
    resource: &DraftResourceRef,
) -> Result<TargetResource, ServerError> {
    let row = if let Some(id) = resource.id.as_deref() {
        sqlx::query(
            "SELECT resource_id, path, name
             FROM resources
             WHERE resource_id = $1 AND org_id = $2 AND scope = $3
               AND (($3 = 'org' AND project_id IS NULL) OR project_id = $4)
               AND status = 'active'
             FOR UPDATE",
        )
        .bind(id)
        .bind(org_id)
        .bind(resource.scope.as_str())
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
    } else if let Some(path) = resource.path.as_deref() {
        sqlx::query(
            "SELECT resource_id, path, name
             FROM resources
             WHERE org_id = $1 AND scope = $2
               AND (($2 = 'org' AND project_id IS NULL) OR project_id = $3)
               AND path = $4
               AND status = 'active'
             FOR UPDATE",
        )
        .bind(org_id)
        .bind(resource.scope.as_str())
        .bind(project_id)
        .bind(path)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        return Err(ServerError::InvalidRequest(
            "operation target requires id or path".to_owned(),
        ));
    }
    .ok_or_else(|| ServerError::not_found("resource", resource.id.as_deref().unwrap_or("path")))?;

    Ok(TargetResource {
        resource_id: row.try_get("resource_id")?,
        path: row.try_get("path")?,
        name: row.try_get("name")?,
    })
}

/// Read the organization owning a required carrying project.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn project_org_id(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<String, ServerError> {
    sqlx::query_scalar::<_, String>("SELECT org_id FROM projects WHERE project_id = $1")
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("project", project_id))
}

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

/// Insert initial organization Memory and return its newly generated resource identity.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn insert_org_context(
    tx: &mut Transaction<'_, Postgres>,
    resource_id: &str,
    org_id: &str,
    path: &str,
    body: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO resources (
            resource_id, org_id, project_id, scope, resource_kind, path, name,
            status, revision, content_hash, body
         )
         VALUES ($1, $2, NULL, 'org', 'memory', $3, $4, 'active', 1, $5, $6)",
    )
    .bind(resource_id)
    .bind(org_id)
    .bind(path)
    .bind(name_from_path(path))
    .bind(content_hash(body))
    .bind(body)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Check for an active organization-owned resource before adding a project selection.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn org_resource_exists(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    resource_id: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1
            FROM resources
            WHERE resource_id = $1 AND org_id = $2
              AND scope = 'org' AND status = 'active'
         )",
    )
    .bind(resource_id)
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Add an organization resource selection without duplicating an existing membership.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn upsert_project_org_selection(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    resource_id: &str,
    revision: i64,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO project_org_resource_selections (project_id, resource_id, revision)
         VALUES ($1, $2, $3)
         ON CONFLICT (project_id, resource_id)
         DO UPDATE SET revision = EXCLUDED.revision,
                       updated_at = now()",
    )
    .bind(project_id)
    .bind(resource_id)
    .bind(revision)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Remove the current selected-resource set before a complete replacement.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn delete_project_org_selections(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<(), ServerError> {
    sqlx::query("DELETE FROM project_org_resource_selections WHERE project_id = $1")
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Read authoritative content, proposal history, selections, and bundles for an organization
/// export.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
pub(crate) async fn export_memory_state(
    pool: &PgPool,
    org_id: &str,
) -> Result<MemoryExport, ServerError> {
    let memories = sqlx::query(
        "SELECT resource_id, scope, project_id, path, name, description, status, org_source,
                content_hash, body, updated_at
         FROM resources
         WHERE org_id = $1 AND status = 'active'
         ORDER BY scope, path, resource_id",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    let memories = memories
        .iter()
        .map(|row| {
            Ok(MemoryExportItem {
                org_source: row
                    .try_get::<Option<sqlx::types::Json<super::dto::OrgMemorySource>>, _>(
                        "org_source",
                    )?
                    .map(|source| source.0),
                memory_id: row.try_get("resource_id")?,
                scope: row.try_get("scope")?,
                project_id: row.try_get("project_id")?,
                path: row.try_get("path")?,
                name: row.try_get("name")?,
                description: row.try_get("description")?,
                status: row.try_get("status")?,
                content_hash: row.try_get("content_hash")?,
                body: row.try_get("body")?,
                updated_at: row
                    .try_get::<time::OffsetDateTime, _>("updated_at")?
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|e| ServerError::InvalidRequest(format!("invalid timestamp: {e}")))?,
            })
        })
        .collect::<Result<Vec<_>, ServerError>>()?;

    let draft_rows = sqlx::query(
        "SELECT d.draft_id, d.project_id, d.title, d.description, d.resource_scope,
                d.target_id, d.path, d.status, d.version
         FROM drafts d
         JOIN projects p ON p.project_id = d.project_id
         WHERE p.org_id = $1
         ORDER BY d.updated_at DESC",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    let mut drafts = Vec::with_capacity(draft_rows.len());
    for row in draft_rows {
        let draft_id: String = row.try_get("draft_id")?;
        let operations = sqlx::query(
            "SELECT operation_id, action, resource_scope, resource_kind,
                    target_id, path, new_path, content
             FROM draft_operations
             WHERE draft_id = $1
             ORDER BY ordinal",
        )
        .bind(&draft_id)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|operation_row| {
            let action: String = operation_row.try_get("action")?;
            let resource_scope: String = operation_row.try_get("resource_scope")?;
            let resource_kind: String = operation_row.try_get("resource_kind")?;
            let target_id: Option<String> = operation_row.try_get("target_id")?;
            let path: Option<String> = operation_row.try_get("path")?;
            let new_path: Option<String> = operation_row.try_get("new_path")?;
            let content: Option<serde_json::Value> = operation_row.try_get("content")?;
            Ok(serde_json::json!({
                "action": action,
                "resource_scope": resource_scope,
                "resource_kind": resource_kind,
                "target_id": target_id,
                "path": path,
                "new_path": new_path,
                "content": content,
            }))
        })
        .collect::<Result<Vec<_>, ServerError>>()?;
        drafts.push(MemoryExportDraft {
            draft_id,
            project_id: row.try_get("project_id")?,
            title: row.try_get("title")?,
            description: row.try_get("description")?,
            resource_scope: row.try_get("resource_scope")?,
            target_id: row.try_get("target_id")?,
            path: row.try_get("path")?,
            status: row.try_get("status")?,
            version: row.try_get("version")?,
            operations,
        });
    }

    let selections = sqlx::query(
        "SELECT s.project_id, s.revision,
                coalesce(array_agg(sr.resource_id ORDER BY sr.resource_id)
                         FILTER (WHERE sr.resource_id IS NOT NULL), '{}') AS resource_ids
         FROM project_org_selection_states s
         LEFT JOIN project_org_resource_selections sr ON sr.project_id = s.project_id
         JOIN projects p ON p.project_id = s.project_id
         WHERE p.org_id = $1
         GROUP BY s.project_id, s.revision",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|row| {
        Ok(MemoryExportSelection {
            project_id: row.try_get("project_id")?,
            resource_ids: row.try_get::<Vec<String>, _>("resource_ids")?,
            revision: row.try_get("revision")?,
        })
    })
    .collect::<Result<Vec<_>, ServerError>>()?;

    let bundles = sqlx::query(
        "SELECT b.bundle_id, b.owner_user_id, b.name, b.description, b.revision,
                coalesce(array_agg(bi.resource_id ORDER BY bi.position)
                         FILTER (WHERE bi.resource_id IS NOT NULL), '{}') AS resource_ids
         FROM personal_bundles b
         LEFT JOIN personal_bundle_items bi ON bi.bundle_id = b.bundle_id
         GROUP BY b.bundle_id
         ORDER BY b.bundle_id",
    )
    .fetch_all(pool)
    .await?
    .iter()
    .map(|row| {
        Ok(MemoryExportBundle {
            bundle_id: row.try_get("bundle_id")?,
            owner_user_id: row.try_get("owner_user_id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            resource_ids: row.try_get::<Vec<String>, _>("resource_ids")?,
            revision: row.try_get("revision")?,
        })
    })
    .collect::<Result<Vec<_>, ServerError>>()?;

    Ok(MemoryExport {
        org_id: org_id.to_owned(),
        exported_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| ServerError::InvalidRequest(format!("invalid timestamp: {e}")))?,
        memories,
        drafts,
        selections,
        bundles,
    })
}

/// Decode active Memory metadata for the requested ownership scope.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_memory_meta(
    pool: &PgPool,
    scope: &str,
    org_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<Vec<MemoryMeta>, ServerError> {
    let rows = list_resource_rows(pool, scope, org_id, project_id).await?;
    rows.iter().map(memory_meta_from_row).collect()
}

/// Decode required scoped Memory content and derive its HTTP validator.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn load_memory_detail(
    tx: &mut Transaction<'_, Postgres>,
    memory_id: &str,
    scope: &str,
    org_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<MemoryDetail, ServerError> {
    let row = load_resource_detail_row(tx, memory_id, scope, org_id, project_id).await?;
    let memory = memory_meta_from_row(&row)?;
    Ok(MemoryDetail {
        content: row.try_get("body")?,
        etag: etag(row.try_get("revision")?),
        memory,
    })
}

/// Persist a new authoritative resource with its content digest and initial version.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn insert_memory(
    tx: &mut Transaction<'_, Postgres>,
    input: NewMemory<'_>,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO resources (
                    resource_id, org_id, project_id, scope, resource_kind, path, name,
                    status, revision, content_hash, body, org_source
                 )
                 VALUES ($1, $2, $3, $4, 'memory', $5, $6, 'active', 1, $7, $8, $9)",
    )
    .bind(input.resource_id)
    .bind(input.org_id)
    .bind(input.project_id)
    .bind(input.scope)
    .bind(input.path)
    .bind(input.name)
    .bind(input.content_hash)
    .bind(input.body)
    .bind(input.org_source.map(sqlx::types::Json))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Replace authoritative content and its digest while advancing the resource version.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn update_memory_content(
    tx: &mut Transaction<'_, Postgres>,
    resource_id: &str,
    name: &str,
    body: &str,
    content_hash: String,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE resources
                 SET name = $2, body = $3, content_hash = $4, revision = revision + 1,
                     status = 'active', updated_at = now()
                 WHERE resource_id = $1",
    )
    .bind(resource_id)
    .bind(name)
    .bind(body)
    .bind(content_hash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Change an authoritative resource path and display name while advancing its version.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn rename_memory(
    tx: &mut Transaction<'_, Postgres>,
    resource_id: &str,
    path: &str,
    name: String,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE resources
                 SET path = $2,
                     name = $3,
                     revision = revision + 1,
                     updated_at = now()
                 WHERE resource_id = $1",
    )
    .bind(resource_id)
    .bind(path)
    .bind(name)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Mark a resource inactive so subsequent authoritative snapshots omit it.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn archive_memory(
    tx: &mut Transaction<'_, Postgres>,
    resource_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE resources
                 SET status = 'archived', revision = revision + 1, updated_at = now()
                 WHERE resource_id = $1",
    )
    .bind(resource_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Select newly published organization Memory into its proposing project.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn select_created_memory(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    resource_id: &str,
    revision: i64,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO project_org_resource_selections (project_id, resource_id, revision)
             VALUES ($1, $2, $3)
             ON CONFLICT (project_id, resource_id)
             DO UPDATE SET revision = EXCLUDED.revision,
                           updated_at = now()",
    )
    .bind(project_id)
    .bind(resource_id)
    .bind(revision)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Find projects selecting any resource affected by an organization publication.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn list_selecting_projects(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    resource_ids: &[String],
) -> Result<Vec<SelectingProject>, ServerError> {
    Ok(sqlx::query_as::<_, SelectingProject>(
        "SELECT DISTINCT s.project_id
         FROM project_org_resource_selections s
         JOIN projects p ON p.project_id = s.project_id
         WHERE p.org_id = $1 AND s.resource_id = ANY($2::text[])
         ORDER BY s.project_id",
    )
    .bind(org_id)
    .bind(resource_ids)
    .fetch_all(&mut **tx)
    .await?)
}

/// Remove selection rows referencing deleted organization resources.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn remove_deleted_selections(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    resource_ids: &[String],
) -> Result<u64, ServerError> {
    Ok(sqlx::query(
        "DELETE FROM project_org_resource_selections
                 WHERE project_id = $1 AND resource_id = ANY($2::text[])",
    )
    .bind(project_id)
    .bind(resource_ids)
    .execute(&mut **tx)
    .await?
    .rows_affected())
}

/// Find a project selection that points outside its owning organization.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn find_cross_org_selection(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
) -> Result<Option<String>, ServerError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT s.resource_id
         FROM project_org_resource_selections s
         JOIN resources r ON r.resource_id = s.resource_id
         WHERE s.project_id = $1 AND r.org_id <> $2
         LIMIT 1",
    )
    .bind(project_id)
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Read project and selected organization paths that form the effective snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn list_effective_paths(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
) -> Result<Vec<EffectiveMemoryPath>, ServerError> {
    Ok(sqlx::query_as::<_, EffectiveMemoryPath>(
        "SELECT r.resource_id, r.resource_kind, r.path
         FROM resources r
         WHERE r.status = 'active'
           AND (
             (r.scope = 'project' AND r.project_id = $1)
             OR (
               r.scope = 'org' AND r.org_id = $2
               AND NOT EXISTS (SELECT 1 FROM resources a WHERE a.project_id = $1
                 AND a.scope = 'project' AND a.status = 'active'
                 AND a.org_source->>'resource_id' = r.resource_id)
               AND EXISTS(
                 SELECT 1
                 FROM project_org_resource_selections s
                 WHERE s.project_id = $1 AND s.resource_id = r.resource_id
               )
             )
           )
         ORDER BY r.path, r.resource_id",
    )
    .bind(project_id)
    .bind(org_id)
    .fetch_all(&mut **tx)
    .await?)
}

/// Project identity and revision affected by changed organization Memory.
#[derive(sqlx::FromRow)]
pub(super) struct SelectingProject {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
}

/// Resource category and output path participating in a project's effective snapshot.
#[derive(sqlx::FromRow)]
pub(super) struct EffectiveMemoryPath {
    /// Stable identity of the persisted resource.
    pub(super) resource_id: String,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub(super) path: String,
}

/// Read active resource metadata selected by a personal bundle.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_bundle_memories(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
) -> Result<Vec<MemoryMeta>, ServerError> {
    let rows = sqlx::query(
        "SELECT
            r.resource_id, r.scope, r.project_id, r.path, r.name, r.description, r.org_source,
            r.status, r.content_hash, r.updated_at
         FROM personal_bundle_items i
         JOIN resources r ON r.resource_id = i.resource_id
         WHERE i.bundle_id = $1 AND r.status = 'active'
         ORDER BY i.position, r.path",
    )
    .bind(bundle_id)
    .fetch_all(&mut **tx)
    .await?;

    let memories = rows
        .iter()
        .map(memory_meta_from_row)
        .collect::<Result<_, _>>()?;

    Ok(memories)
}

/// Validated resource ownership, identity, path, and content for authoritative persistence.
pub(super) struct NewMemory<'a> {
    /// Optional immutable source of a Project adaptation.
    pub(super) org_source: Option<&'a super::dto::OrgMemorySource>,
    /// Stable identity of the persisted resource.
    pub(super) resource_id: &'a str,
    /// Organization boundary to which the resource or identity belongs.
    pub(super) org_id: &'a str,
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: Option<&'a str>,
    /// Ownership boundary determining which reference and resource set apply.
    pub(super) scope: &'a str,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub(super) path: &'a str,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub(super) name: &'a str,
    /// Fingerprint used to detect resource content changes.
    pub(super) content_hash: String,
    /// Stored text content, including Markdown where the resource contract permits it.
    pub(super) body: &'a str,
}

/// Bind an adaptation to an Organization entry in this project's immutable snapshot.
///
/// # Errors
/// Rejects forged sources, snapshots from another project, and missing entries.
pub(crate) async fn validate_org_source(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    source: &crate::app::memory::dto::OrgMemorySource,
) -> Result<(), ServerError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commits c JOIN tree_entries e ON e.tree_id = c.tree_id
         WHERE c.commit_id = $1 AND c.scope = 'project' AND c.project_id = $2
           AND e.item_id = $3 AND e.scope = 'org' AND e.resource_kind = 'memory')",
    )
    .bind(&source.commit_id)
    .bind(project_id)
    .bind(&source.resource_id)
    .fetch_one(&mut **tx)
    .await?;
    if !valid {
        return Err(ServerError::InvalidRequest(
            "adaptation source must be selected Organization Memory in this Project snapshot"
                .to_owned(),
        ));
    }
    Ok(())
}
