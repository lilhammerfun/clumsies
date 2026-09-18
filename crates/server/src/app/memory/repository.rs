//! SQL queries and persistence for memory resources.

use super::model::{OrgResourceImpact, TargetResource, prepare_resource_content, resource_status};
use crate::app::bundle::dto::MemoryExportBundle;
use crate::app::commit::service::{
    advance_project_ref, create_project_commit, current_project_ref,
};
use crate::app::draft::dto::{DraftOperationAction, DraftOperationInput, DraftResourceRef};
use crate::app::memory::dto::{
    MemoryExport, MemoryExportDraft, MemoryExportItem, MemoryExportSelection, MemoryMeta,
    ProjectOrgSelection, ResourceScope,
};
use crate::app::memory::model::{
    content_hash, insert_materialization_path, materialization_output_path, name_from_path,
    resource_scope, validate_resource_path,
};
use crate::app::organization::dto::UserRef;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet};

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

pub(crate) async fn ensure_removed_org_resources_have_no_active_drafts(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    retained_resource_ids: &[String],
) -> Result<(), ServerError> {
    let blocked = sqlx::query(
        "WITH removed AS (
            SELECT selection.resource_id, resource.org_id
            FROM project_org_resource_selections AS selection
            JOIN resources AS resource
              ON resource.resource_id = selection.resource_id
            WHERE selection.project_id = $1
              AND NOT (selection.resource_id = ANY($2))
         ), active_drafts AS (
            SELECT draft.draft_id, draft.base_commit_id, draft.created_at,
                   draft.target_id, draft.path
            FROM drafts AS draft
            WHERE draft.project_id = $1
              AND draft.resource_scope = 'org'
              AND draft.status IN ('open', 'submitted')
              AND NOT COALESCE((
                    SELECT operation.action = 'create'
                    FROM draft_operations AS operation
                    WHERE operation.draft_id = draft.draft_id
                    ORDER BY operation.ordinal
                    LIMIT 1
              ), FALSE)
         ), targets AS (
            SELECT draft_id, base_commit_id, created_at, target_id, path
            FROM active_drafts
            UNION ALL
            SELECT draft.draft_id, draft.base_commit_id, draft.created_at,
                   operation.target_id, operation.path
            FROM active_drafts AS draft
            JOIN draft_operations AS operation
              ON operation.draft_id = draft.draft_id
            WHERE operation.resource_scope = 'org'
              AND operation.action <> 'create'
         )
         SELECT removed.resource_id, target.draft_id
         FROM removed
         JOIN targets AS target
           ON COALESCE(
                target.target_id,
                (
                    SELECT base_entry.item_id
                    FROM commits AS base_commit
                    JOIN tree_entries AS base_entry
                      ON base_entry.tree_id = base_commit.tree_id
                    WHERE base_commit.commit_id = target.base_commit_id
                      AND base_commit.org_id = removed.org_id
                      AND base_commit.scope = 'org'
                      AND base_entry.scope = 'org'
                      AND base_entry.resource_kind = 'memory'
                      AND base_entry.path = target.path
                ),
                (
                    SELECT resource.resource_id
                    FROM resources AS resource
                    WHERE resource.org_id = removed.org_id
                      AND resource.scope = 'org'
                      AND resource.status = 'active'
                      AND resource.path = target.path
                ),
                (
                    SELECT CASE
                        WHEN COUNT(DISTINCT historical_entry.item_id) = 1
                        THEN MIN(historical_entry.item_id)
                    END
                    FROM commits AS historical_commit
                    JOIN tree_entries AS historical_entry
                      ON historical_entry.tree_id = historical_commit.tree_id
                    WHERE historical_commit.org_id = removed.org_id
                      AND historical_commit.scope = 'org'
                      AND historical_entry.scope = 'org'
                      AND historical_entry.resource_kind = 'memory'
                      AND historical_entry.path = target.path
                )
              ) = removed.resource_id
         ORDER BY removed.resource_id, target.created_at, target.draft_id
         LIMIT 1",
    )
    .bind(project_id)
    .bind(retained_resource_ids)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(blocked) = blocked else {
        return Ok(());
    };
    let resource_id: String = blocked.try_get("resource_id")?;
    let draft_id: String = blocked.try_get("draft_id")?;
    Err(ServerError::InvalidRequest(format!(
        "cannot remove Organization Memory {resource_id} from this Project while active Organization Draft {draft_id} targets it; discard or finish the Draft first"
    )))
}

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

pub(crate) async fn list_resource_rows(
    pool: &PgPool,
    scope: &str,
    org_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<Vec<sqlx::postgres::PgRow>, ServerError> {
    let rows = if let Some(project_id) = project_id {
        sqlx::query(
            "SELECT
                resource_id, scope, project_id, path, name, description, status,
                content_hash, updated_at
             FROM resources
             WHERE scope = $1 AND project_id = $2 AND status = 'active'
             ORDER BY path
             LIMIT 200",
        )
        .bind(scope)
        .bind(project_id)
        .fetch_all(pool)
        .await?
    } else if let Some(org_id) = org_id {
        sqlx::query(
            "SELECT
                resource_id, scope, project_id, path, name, description, status,
                content_hash, updated_at
             FROM resources
             WHERE scope = $1 AND org_id = $2 AND status = 'active'
             ORDER BY path
             LIMIT 200",
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
                resource_id, scope, project_id, path, name, description, status,
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
                resource_id, scope, project_id, path, name, description, status,
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

pub(crate) fn memory_meta_from_row(row: &sqlx::postgres::PgRow) -> Result<MemoryMeta, ServerError> {
    Ok(MemoryMeta {
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

pub(crate) async fn apply_resource_operation(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    scope: ResourceScope,
    operation: &DraftOperationInput,
) -> Result<Option<String>, ServerError> {
    let org_id = project_org_id(tx, project_id).await?;
    let resource_project_id = (scope == ResourceScope::Project).then_some(project_id);
    match operation.action {
        DraftOperationAction::Create => {
            let path = operation.resource.path.as_ref().ok_or_else(|| {
                ServerError::InvalidRequest("create operation requires path".to_owned())
            })?;
            let content = operation.content.as_ref().ok_or_else(|| {
                ServerError::InvalidRequest("create operation requires content".to_owned())
            })?;
            let prepared = prepare_resource_content(path, content, None)?;
            let resource_id = prefixed_id("mem");
            sqlx::query(
                "INSERT INTO resources (
                    resource_id, org_id, project_id, scope, resource_kind, path, name,
                    status, revision, content_hash, body
                 )
                 VALUES ($1, $2, $3, $4, 'memory', $5, $6, 'active', 1, $7, $8)",
            )
            .bind(&resource_id)
            .bind(&org_id)
            .bind(resource_project_id)
            .bind(scope.as_str())
            .bind(path)
            .bind(&prepared.name)
            .bind(content_hash(&prepared.body))
            .bind(&prepared.body)
            .execute(&mut **tx)
            .await?;
            return Ok(Some(resource_id));
        }
        DraftOperationAction::Update => {
            let resource =
                load_target_resource(tx, &org_id, resource_project_id, &operation.resource).await?;
            let content = operation.content.as_ref().ok_or_else(|| {
                ServerError::InvalidRequest("update operation requires content".to_owned())
            })?;
            let prepared = prepare_resource_content(&resource.path, content, Some(&resource))?;
            sqlx::query(
                "UPDATE resources
                 SET name = $2, body = $3, content_hash = $4, revision = revision + 1,
                     status = 'active', updated_at = now()
                 WHERE resource_id = $1",
            )
            .bind(&resource.resource_id)
            .bind(&prepared.name)
            .bind(&prepared.body)
            .bind(content_hash(&prepared.body))
            .execute(&mut **tx)
            .await?;
        }
        DraftOperationAction::Rename => {
            let resource =
                load_target_resource(tx, &org_id, resource_project_id, &operation.resource).await?;
            let new_path = operation.new_path.as_ref().ok_or_else(|| {
                ServerError::InvalidRequest("rename operation requires new_path".to_owned())
            })?;
            sqlx::query(
                "UPDATE resources
                 SET path = $2,
                     name = $3,
                     revision = revision + 1,
                     updated_at = now()
                 WHERE resource_id = $1",
            )
            .bind(&resource.resource_id)
            .bind(new_path)
            .bind(name_from_path(new_path))
            .execute(&mut **tx)
            .await?;
        }
        DraftOperationAction::Delete => {
            let resource =
                load_target_resource(tx, &org_id, resource_project_id, &operation.resource).await?;
            sqlx::query(
                "UPDATE resources
                 SET status = 'archived', revision = revision + 1, updated_at = now()
                 WHERE resource_id = $1",
            )
            .bind(&resource.resource_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(None)
}

/// A new Organization Memory proposed from a Project becomes part of that
/// Project's effective Memory when the Review merges. Existing Organization
/// resources keep their explicit Add/Remove membership semantics.
pub(crate) async fn select_created_org_resources_for_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
    resource_ids: &[String],
) -> Result<(), ServerError> {
    let parent_commit_id = current_project_ref(tx, project_id).await?;
    let current_revision = current_project_org_selection_revision(tx, project_id).await?;
    let next_revision = current_revision + 1;
    for resource_id in resource_ids {
        sqlx::query(
            "INSERT INTO project_org_resource_selections (project_id, resource_id, revision)
             VALUES ($1, $2, $3)
             ON CONFLICT (project_id, resource_id)
             DO UPDATE SET revision = EXCLUDED.revision,
                           updated_at = now()",
        )
        .bind(project_id)
        .bind(resource_id)
        .bind(next_revision)
        .execute(&mut **tx)
        .await?;
    }
    validate_project_effective_memory(tx, project_id, org_id).await?;
    update_project_org_selection_revision(tx, project_id, next_revision).await?;
    let commit_id = create_project_commit(tx, project_id, parent_commit_id.as_deref()).await?;
    advance_project_ref(tx, project_id, &commit_id).await?;
    Ok(())
}

pub(crate) async fn refresh_projects_for_org_resource_changes(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    impact: &OrgResourceImpact,
) -> Result<(), ServerError> {
    if impact.resource_ids.is_empty() {
        return Ok(());
    }

    let resource_ids = impact.resource_ids.iter().cloned().collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT DISTINCT s.project_id
         FROM project_org_resource_selections s
         JOIN projects p ON p.project_id = s.project_id
         WHERE p.org_id = $1 AND s.resource_id = ANY($2::text[])
         ORDER BY s.project_id",
    )
    .bind(org_id)
    .bind(&resource_ids)
    .fetch_all(&mut **tx)
    .await?;
    let deleted_resource_ids = impact
        .deleted_resource_ids
        .iter()
        .cloned()
        .collect::<Vec<_>>();

    for row in rows {
        let project_id: String = row.try_get("project_id")?;
        let parent_commit_id = current_project_ref(tx, &project_id).await?;
        if !deleted_resource_ids.is_empty() {
            let deleted = sqlx::query(
                "DELETE FROM project_org_resource_selections
                 WHERE project_id = $1 AND resource_id = ANY($2::text[])",
            )
            .bind(&project_id)
            .bind(&deleted_resource_ids)
            .execute(&mut **tx)
            .await?;
            if deleted.rows_affected() > 0 {
                let revision = current_project_org_selection_revision(tx, &project_id).await?;
                update_project_org_selection_revision(tx, &project_id, revision + 1).await?;
            }
        }
        let commit_id = create_project_commit(tx, &project_id, parent_commit_id.as_deref()).await?;
        advance_project_ref(tx, &project_id, &commit_id).await?;
    }
    Ok(())
}

pub(crate) async fn validate_project_effective_memory(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
) -> Result<(), ServerError> {
    let cross_org_resource = sqlx::query_scalar::<_, String>(
        "SELECT s.resource_id
         FROM project_org_resource_selections s
         JOIN resources r ON r.resource_id = s.resource_id
         WHERE s.project_id = $1 AND r.org_id <> $2
         LIMIT 1",
    )
    .bind(project_id)
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(resource_id) = cross_org_resource {
        return Err(ServerError::InvalidRequest(format!(
            "project cannot select a resource from another organization: {resource_id}"
        )));
    }

    let rows = sqlx::query(
        "SELECT r.resource_id, r.resource_kind, r.path
         FROM resources r
         WHERE r.status = 'active'
           AND (
             (r.scope = 'project' AND r.project_id = $1)
             OR (
               r.scope = 'org' AND r.org_id = $2
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
    .await?;
    let mut output_paths = BTreeMap::new();
    for row in rows {
        let resource_id: String = row.try_get("resource_id")?;
        let path: String = row.try_get("path")?;
        validate_resource_path(&path)?;
        let output_path = materialization_output_path(&path)?;
        insert_materialization_path(
            &mut output_paths,
            &resource_id,
            &output_path,
            "project effective memory",
        )?;
    }

    Ok(())
}

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
            r.resource_id, r.scope, r.project_id, r.path, r.name, r.description,
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

pub(crate) async fn export_memory_state(
    pool: &PgPool,
    org_id: &str,
) -> Result<MemoryExport, ServerError> {
    let memories = sqlx::query(
        "SELECT resource_id, scope, project_id, path, name, description, status,
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
