//! Commit use cases and transaction coordination.

use super::repository::{create_commit, store_tree};
use crate::app::auth::AuthPrincipal;
use crate::app::commit::dto::{CommitListResponse, CommitPayload, CommitStateResponse};
use crate::app::commit::model::{PendingTreeEntry, validate_tree_materialization_paths};
use crate::app::commit::repository;
use crate::app::commit::repository::store_blob;
use crate::app::memory::{
    load_project_org_selection, pending_resource_entry, project_org_id,
    validate_project_effective_memory,
};
use crate::app::{memory, project};
use crate::error::ServerError;
use sqlx::{Postgres, Transaction};

/// Hide snapshots outside the principal's organization or project membership.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
async fn ensure_commit_access(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    commit_id: &str,
) -> Result<(), ServerError> {
    if repository::commit_is_accessible(pool, principal, commit_id).await? {
        Ok(())
    } else {
        Err(ServerError::not_found("commit", commit_id))
    }
}

/// Return project snapshot history after enforcing project membership.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_project_commits(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
) -> Result<CommitListResponse, ServerError> {
    project::ensure_project_member(pool, principal, project_id).await?;

    repository::list_project_commits(pool, project_id).await
}

/// Return a complete immutable snapshot only to an identity authorized for its scope.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_commit_payload(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    commit_id: &str,
) -> Result<CommitPayload, ServerError> {
    ensure_commit_access(pool, principal, commit_id).await?;

    let mut tx = pool.begin().await?;
    let payload = repository::load_commit_payload(&mut tx, commit_id).await?;
    tx.commit().await?;
    Ok(payload)
}

/// Compare the client's snapshot with the current head of an accessible project.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_project_commit_state(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    local_commit_id: Option<&str>,
) -> Result<CommitStateResponse, ServerError> {
    project::ensure_project_member(pool, principal, project_id).await?;

    let mut tx = pool.begin().await?;
    memory::project_org_id(&mut tx, project_id).await?;
    let reference = repository::load_project_ref(&mut tx, project_id).await?;
    let latest = match reference.commit_id.as_deref() {
        Some(commit_id) => Some(repository::load_commit_metadata(&mut tx, commit_id).await?),
        None => None,
    };
    tx.commit().await?;
    Ok(CommitStateResponse {
        update_available: local_commit_id != reference.commit_id.as_deref(),
        download_url: reference
            .commit_id
            .as_ref()
            .map(|commit_id| format!("/api/v1/commits/{commit_id}")),
        reference,
        latest,
        incremental_supported: false,
    })
}

/// Return snapshot history within the authenticated identity's organization.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_org_commits(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
) -> Result<CommitListResponse, ServerError> {
    let org_id = &principal.org_id;

    repository::list_org_commits(pool, org_id).await
}

/// Compare the client's organization snapshot with the current authoritative head.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_org_commit_state(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    local_commit_id: Option<&str>,
) -> Result<CommitStateResponse, ServerError> {
    let org_id = &principal.org_id;

    let mut tx = pool.begin().await?;
    let reference = repository::load_org_ref(&mut tx, org_id).await?;
    let latest = match reference.commit_id.as_deref() {
        Some(commit_id) => Some(repository::load_commit_metadata(&mut tx, commit_id).await?),
        None => None,
    };
    tx.commit().await?;
    Ok(CommitStateResponse {
        update_available: local_commit_id != reference.commit_id.as_deref(),
        download_url: reference
            .commit_id
            .as_ref()
            .map(|commit_id| format!("/api/v1/commits/{commit_id}")),
        reference,
        latest,
        incremental_supported: false,
    })
}

// Transaction participants share the caller-owned transaction; only the outer service commits.

/// Materialize project and selected organization Memory into one immutable snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
pub(crate) async fn create_project_commit(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    parent_commit_id: Option<&str>,
) -> Result<String, ServerError> {
    let org_id = project_org_id(tx, project_id).await?;
    validate_project_effective_memory(tx, project_id, &org_id).await?;
    let version = repository::latest_project_version(tx, project_id)
        .await?
        .unwrap_or(0)
        + 1;
    let mut entries = Vec::new();
    let project_rows = repository::project_snapshot_resources(tx, project_id).await?;
    for row in project_rows {
        entries
            .push(pending_resource_entry(tx, &row, "project", Some(project_id), "project").await?);
    }

    let selected_rows = repository::selected_snapshot_resources(tx, project_id).await?;
    for row in selected_rows {
        entries.push(pending_resource_entry(tx, &row, "org", None, "selected_org").await?);
    }

    let project_org_selection = load_project_org_selection(tx, project_id).await?;
    let selection_content = serde_json::to_string(&project_org_selection).map_err(|error| {
        ServerError::InvalidRequest(format!(
            "failed to serialize project org selection: {error}"
        ))
    })?;
    let selection_blob_id = store_blob(tx, &selection_content).await?;
    entries.push(PendingTreeEntry {
        org_source: None,
        item_id: format!("project_org_selection:{project_id}"),
        resource_kind: "project_org_selection".to_owned(),
        scope: "daemon".to_owned(),
        project_id: Some(project_id.to_owned()),
        path: None,
        blob_id: selection_blob_id,
        source: "config".to_owned(),
        description: String::new(),
    });

    validate_tree_materialization_paths(&entries)?;
    let tree_id = store_tree(tx, &entries).await?;
    create_commit(
        tx,
        "project",
        &org_id,
        Some(project_id),
        &tree_id,
        parent_commit_id,
        version,
    )
    .await
}

/// Materialize active organization Memory into an immutable authoritative snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
pub(crate) async fn create_org_commit(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    parent_commit_id: Option<&str>,
) -> Result<String, ServerError> {
    let version = repository::latest_org_version(tx, org_id)
        .await?
        .unwrap_or(0)
        + 1;
    let rows = repository::org_snapshot_resources(tx, org_id).await?;
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        entries.push(pending_resource_entry(tx, &row, "org", None, "org").await?);
    }
    validate_tree_materialization_paths(&entries)?;
    let tree_id = store_tree(tx, &entries).await?;
    create_commit(tx, "org", org_id, None, &tree_id, parent_commit_id, version).await
}
