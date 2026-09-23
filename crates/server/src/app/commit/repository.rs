//! SQL queries and persistence for commit resources.

use super::model::{
    PendingTreeEntry, commit_scope, tree_entry_kind, tree_entry_scope, tree_entry_source,
};
use crate::app::auth::AuthPrincipal;
use crate::app::commit::dto::{
    Blob, Commit, CommitListResponse, CommitPayload, Ref, Tree, TreeEntry, TreeEntryKind,
};
use crate::app::memory::model::object_id;
use crate::error::ServerError;
use crate::pagination::{PageInfo, page_info};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::BTreeMap;
use time::OffsetDateTime;

/// Persist a content-addressed payload once and return its stable identifier.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn store_blob(
    tx: &mut Transaction<'_, Postgres>,
    content: &str,
) -> Result<String, ServerError> {
    let blob_id = object_id("blob", content.as_bytes());
    sqlx::query("INSERT INTO blobs (blob_id, content) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(&blob_id)
        .bind(content)
        .execute(&mut **tx)
        .await?;
    Ok(blob_id)
}

/// Persist a canonically ordered content-addressed tree and its resource entries.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
pub(crate) async fn store_tree(
    tx: &mut Transaction<'_, Postgres>,
    entries: &[PendingTreeEntry],
) -> Result<String, ServerError> {
    let mut canonical_entries = entries.iter().collect::<Vec<_>>();
    canonical_entries.sort_by(|left, right| {
        left.resource_kind
            .cmp(&right.resource_kind)
            .then_with(|| match (&left.path, &right.path) {
                (Some(left), Some(right)) => left.cmp(right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| left.item_id.cmp(&right.item_id))
    });
    let encoded = serde_json::to_vec(&canonical_entries)
        .map_err(|error| ServerError::InvalidRequest(format!("failed to encode tree: {error}")))?;
    let tree_id = object_id("tree", &encoded);
    sqlx::query("INSERT INTO trees (tree_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(&tree_id)
        .execute(&mut **tx)
        .await?;
    for entry in entries {
        sqlx::query(
            "INSERT INTO tree_entries (
                tree_id, item_id, resource_kind, scope, project_id, path, blob_id, source,
                description, org_source
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT DO NOTHING",
        )
        .bind(&tree_id)
        .bind(&entry.item_id)
        .bind(&entry.resource_kind)
        .bind(&entry.scope)
        .bind(&entry.project_id)
        .bind(&entry.path)
        .bind(&entry.blob_id)
        .bind(&entry.source)
        .bind(&entry.description)
        .bind(entry.org_source.as_ref().map(sqlx::types::Json))
        .execute(&mut **tx)
        .await?;
    }
    Ok(tree_id)
}

/// Persist immutable snapshot metadata linking a tree to its parent reference.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
pub(crate) async fn create_commit(
    tx: &mut Transaction<'_, Postgres>,
    scope: &str,
    org_id: &str,
    project_id: Option<&str>,
    tree_id: &str,
    parent_commit_id: Option<&str>,
    version: i64,
) -> Result<String, ServerError> {
    let created_at = OffsetDateTime::now_utc();
    let encoded = serde_json::to_vec(&(
        scope,
        org_id,
        project_id,
        tree_id,
        parent_commit_id,
        version,
        created_at.unix_timestamp_nanos(),
    ))
    .map_err(|error| ServerError::InvalidRequest(format!("failed to encode commit: {error}")))?;
    let commit_id = object_id("commit", &encoded);
    sqlx::query(
        "INSERT INTO commits (
            commit_id, scope, org_id, project_id, tree_id, parent_commit_id, version, created_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&commit_id)
    .bind(scope)
    .bind(org_id)
    .bind(project_id)
    .bind(tree_id)
    .bind(parent_commit_id)
    .bind(version)
    .bind(created_at)
    .execute(&mut **tx)
    .await?;
    Ok(commit_id)
}

/// Decode snapshot metadata and author identity from a joined database row.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) fn commit_from_row(row: &sqlx::postgres::PgRow) -> Result<Commit, ServerError> {
    Ok(Commit {
        commit_id: row.try_get("commit_id")?,
        scope: commit_scope(row.try_get::<String, _>("scope")?.as_str())?,
        org_id: row.try_get("org_id")?,
        project_id: row.try_get("project_id")?,
        tree_id: row.try_get("tree_id")?,
        parent_commit_id: row.try_get("parent_commit_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Load immutable metadata for a required snapshot identity.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_commit_metadata(
    tx: &mut Transaction<'_, Postgres>,
    commit_id: &str,
) -> Result<Commit, ServerError> {
    let row = sqlx::query(
        "SELECT commit_id, scope, org_id, project_id, tree_id, parent_commit_id, version, created_at
         FROM commits
         WHERE commit_id = $1",
    )
    .bind(commit_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("commit", commit_id))?;
    commit_from_row(&row)
}

/// Assemble stored snapshot metadata, ordered tree entries, and referenced blobs.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
pub(crate) async fn load_commit_payload(
    tx: &mut Transaction<'_, Postgres>,
    commit_id: &str,
) -> Result<CommitPayload, ServerError> {
    let commit = load_commit_metadata(tx, commit_id).await?;
    let item_rows = sqlx::query(
        "SELECT e.item_id, e.resource_kind, e.scope, e.project_id, e.path, e.blob_id,
                e.source, e.description, e.org_source, b.content
         FROM tree_entries e
         JOIN blobs b ON b.blob_id = e.blob_id
         WHERE e.tree_id = $1
         ORDER BY e.resource_kind, e.path NULLS LAST, e.item_id",
    )
    .bind(&commit.tree_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut tree_entries = Vec::with_capacity(item_rows.len());
    let mut blobs = BTreeMap::new();
    let mut project_org_selection = None;
    for row in item_rows {
        let kind = tree_entry_kind(row.try_get::<String, _>("resource_kind")?.as_str())?;
        let scope = tree_entry_scope(row.try_get::<String, _>("scope")?.as_str())?;
        let source = tree_entry_source(row.try_get::<String, _>("source")?.as_str())?;
        let id: String = row.try_get("item_id")?;
        let project_id: Option<String> = row.try_get("project_id")?;
        let path: Option<String> = row.try_get("path")?;
        let blob_id: String = row.try_get("blob_id")?;
        let content: String = row.try_get("content")?;
        tree_entries.push(TreeEntry {
            org_source: row
                .try_get::<Option<sqlx::types::Json<crate::app::memory::dto::OrgMemorySource>>, _>(
                    "org_source",
                )?
                .map(|source| source.0),
            id,
            kind,
            scope,
            project_id,
            path,
            blob_id: blob_id.clone(),
            source,
            description: row.try_get("description")?,
        });
        if kind == TreeEntryKind::ProjectOrgSelection {
            project_org_selection = Some(serde_json::from_str(&content).map_err(|error| {
                ServerError::InvalidRequest(format!(
                    "commit project org selection is invalid: {error}"
                ))
            })?);
        }
        blobs
            .entry(blob_id.clone())
            .or_insert(Blob { blob_id, content });
    }

    if commit.project_id.is_some() && project_org_selection.is_none() {
        return Err(ServerError::InvalidRequest(
            "project commit missing project org selection".to_owned(),
        ));
    }

    Ok(CommitPayload {
        tree: Tree {
            tree_id: commit.tree_id.clone(),
            entries: tree_entries,
        },
        commit,
        blobs: blobs.into_values().collect(),
        project_org_selection,
    })
}

/// Read the current project head using the caller's transaction.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn current_project_ref(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<Option<String>, ServerError> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT commit_id
         FROM refs
         WHERE scope = 'project' AND project_id = $1 AND ref_name = 'refs/heads/main'
         FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("ref", project_id))
}

/// Replace the project's main head and advance its reference revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn advance_project_ref(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    commit_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE refs
         SET commit_id = $2, updated_at = now()
         WHERE scope = 'project' AND project_id = $1 AND ref_name = 'refs/heads/main'",
    )
    .bind(project_id)
    .bind(commit_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE draft_reconciliation_candidates c
         SET invalidated_at = now()
         FROM drafts d
         WHERE c.draft_id = d.draft_id
           AND d.project_id = $1
           AND d.resource_scope = 'project'
           AND c.invalidated_at IS NULL",
    )
    .bind(project_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Require a supplied ancestor to belong to the specified project.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
pub(crate) async fn validate_project_commit(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    commit_id: &str,
) -> Result<(), ServerError> {
    let belongs_to_project = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM commits
            WHERE commit_id = $1 AND scope = 'project' AND project_id = $2
         )",
    )
    .bind(commit_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    if !belongs_to_project {
        return Err(ServerError::InvalidRequest(format!(
            "base commit {commit_id} does not belong to project {project_id}"
        )));
    }
    Ok(())
}

/// Require a supplied ancestor to belong to the specified organization.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
pub(crate) async fn validate_org_commit(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    commit_id: &str,
) -> Result<(), ServerError> {
    let belongs_to_org = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM commits
            WHERE commit_id = $1 AND scope = 'org' AND org_id = $2
         )",
    )
    .bind(commit_id)
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await?;
    if !belongs_to_org {
        return Err(ServerError::InvalidRequest(format!(
            "base commit {commit_id} does not belong to organization {org_id}"
        )));
    }
    Ok(())
}

/// Read the current organization head using the caller's transaction.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn current_org_ref(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
) -> Result<Option<String>, ServerError> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT commit_id
         FROM refs
         WHERE scope = 'org' AND org_id = $1 AND ref_name = 'refs/heads/main'
         FOR UPDATE",
    )
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("ref", org_id))
}

/// Lock the organization reference before deriving a project's selected-resource snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn lock_org_ref_for_project_projection(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
) -> Result<(), ServerError> {
    sqlx::query_scalar::<_, String>(
        "SELECT ref_id
         FROM refs
         WHERE scope = 'org' AND org_id = $1 AND ref_name = 'refs/heads/main'
         FOR SHARE",
    )
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("ref", org_id))?;
    Ok(())
}

/// Replace the organization's main head and advance its reference revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn advance_org_ref(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    commit_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE refs
         SET commit_id = $2, updated_at = now()
         WHERE scope = 'org' AND org_id = $1 AND ref_name = 'refs/heads/main'",
    )
    .bind(org_id)
    .bind(commit_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE draft_reconciliation_candidates c
         SET invalidated_at = now()
         FROM drafts d
         JOIN projects p ON p.project_id = d.project_id
         WHERE c.draft_id = d.draft_id
           AND p.org_id = $1
           AND d.resource_scope = 'org'
           AND c.invalidated_at IS NULL",
    )
    .bind(org_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Read the public project-reference representation from its stored row.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_project_ref(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<Ref, ServerError> {
    let row = sqlx::query(
        "SELECT ref_name, scope, org_id, project_id, commit_id, updated_at
         FROM refs
         WHERE scope = 'project' AND project_id = $1 AND ref_name = 'refs/heads/main'",
    )
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("ref", project_id))?;
    ref_from_row(&row)
}

/// Read the organization's main snapshot reference and its concurrency metadata.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_org_ref(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
) -> Result<Ref, ServerError> {
    let row = sqlx::query(
        "SELECT ref_name, scope, org_id, project_id, commit_id, updated_at
         FROM refs
         WHERE scope = 'org' AND org_id = $1 AND ref_name = 'refs/heads/main'",
    )
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("ref", org_id))?;
    ref_from_row(&row)
}

/// Decode a named snapshot reference and its optimistic concurrency metadata.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) fn ref_from_row(row: &sqlx::postgres::PgRow) -> Result<Ref, ServerError> {
    Ok(Ref {
        name: row.try_get("ref_name")?,
        scope: commit_scope(row.try_get::<String, _>("scope")?.as_str())?,
        org_id: row.try_get("org_id")?,
        project_id: row.try_get("project_id")?,
        commit_id: row.try_get("commit_id")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Check snapshot scope against organization identity and explicit project membership.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn commit_is_accessible(
    pool: &PgPool,
    principal: &AuthPrincipal,
    commit_id: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM commits c
            LEFT JOIN project_members m
              ON m.project_id = c.project_id AND m.user_id = $3
            WHERE c.commit_id = $1
              AND c.org_id = $2
              AND (c.scope = 'org' OR m.user_id IS NOT NULL)
         )",
    )
    .bind(commit_id)
    .bind(&principal.org_id)
    .bind(&principal.user_id)
    .fetch_one(pool)
    .await?)
}

/// Read immutable snapshot history for the supplied project identity.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_project_commits(
    pool: &PgPool,
    project_id: &str,
) -> Result<CommitListResponse, ServerError> {
    let rows = sqlx::query(
        "SELECT commit_id, scope, org_id, project_id, tree_id, parent_commit_id,
                version, created_at
         FROM commits
         WHERE scope = 'project' AND project_id = $1
         ORDER BY version DESC
         LIMIT 50",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    let items = rows.iter().map(commit_from_row).collect::<Result<_, _>>()?;

    Ok(CommitListResponse {
        items,
        page_info: PageInfo {
            next_cursor: None,
            has_more: false,
        },
    })
}

/// Read immutable snapshot history for the supplied organization identity.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_org_commits(
    pool: &PgPool,
    org_id: &str,
) -> Result<CommitListResponse, ServerError> {
    let rows = sqlx::query(
        "SELECT commit_id, scope, org_id, project_id, tree_id, parent_commit_id,
                version, created_at
         FROM commits
         WHERE scope = 'org' AND org_id = $1
         ORDER BY version DESC
         LIMIT 50",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    let items = rows.iter().map(commit_from_row).collect::<Result<_, _>>()?;
    Ok(CommitListResponse {
        items,
        page_info: page_info(),
    })
}

/// Read the project's latest snapshot sequence, treating an empty history as zero.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn latest_project_version(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<Option<i64>, ServerError> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(version)
         FROM commits
         WHERE scope = 'project' AND project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Read active project-owned Memory needed to materialize a snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn project_snapshot_resources(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<Vec<crate::app::memory::model::CommitResource>, ServerError> {
    Ok(
        sqlx::query_as::<_, crate::app::memory::model::CommitResource>(
            "SELECT resource_id, resource_kind, path, name, body, description, org_source
         FROM resources
         WHERE scope = 'project' AND project_id = $1 AND status = 'active'
         ORDER BY resource_kind, path",
        )
        .bind(project_id)
        .fetch_all(&mut **tx)
        .await?,
    )
}

/// Read active selected organization Memory needed for a project's effective snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn selected_snapshot_resources(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<Vec<crate::app::memory::model::CommitResource>, ServerError> {
    Ok(
        sqlx::query_as::<_, crate::app::memory::model::CommitResource>(
            "SELECT r.resource_id, r.resource_kind, r.path, r.name, r.body, r.description, r.org_source
         FROM project_org_resource_selections s
         JOIN resources r ON r.resource_id = s.resource_id
         WHERE s.project_id = $1 AND r.status = 'active'
           AND NOT EXISTS (SELECT 1 FROM resources a WHERE a.project_id = $1
             AND a.scope = 'project' AND a.status = 'active'
             AND a.org_source->>'resource_id' = r.resource_id)
         ORDER BY r.resource_kind, r.path",
        )
        .bind(project_id)
        .fetch_all(&mut **tx)
        .await?,
    )
}

/// Read the organization's latest snapshot sequence, treating an empty history as zero.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn latest_org_version(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
) -> Result<Option<i64>, ServerError> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(version) FROM commits WHERE scope = 'org' AND org_id = $1",
    )
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Read active organization-owned Memory needed to materialize its authoritative snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn org_snapshot_resources(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
) -> Result<Vec<crate::app::memory::model::CommitResource>, ServerError> {
    Ok(
        sqlx::query_as::<_, crate::app::memory::model::CommitResource>(
            "SELECT resource_id, resource_kind, path, name, body, description, org_source
         FROM resources
         WHERE scope = 'org' AND org_id = $1 AND status = 'active'
         ORDER BY resource_kind, path",
        )
        .bind(org_id)
        .fetch_all(&mut **tx)
        .await?,
    )
}
