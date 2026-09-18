//! Application operations and transaction coordination for commit resources.

use super::repository;
use crate::app::auth::AuthPrincipal;
use crate::app::commit::dto::{CommitListResponse, CommitPayload, CommitStateResponse};
use crate::app::memory;
use crate::error::ServerError;

pub async fn ensure_commit_access(
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

pub async fn list_project_commits(
    pool: &sqlx::PgPool,
    project_id: &str,
) -> Result<CommitListResponse, ServerError> {
    repository::list_project_commits(pool, project_id).await
}

pub async fn get_commit_payload(
    pool: &sqlx::PgPool,
    commit_id: &str,
) -> Result<CommitPayload, ServerError> {
    let mut tx = pool.begin().await?;
    let payload = repository::load_commit_payload(&mut tx, commit_id).await?;
    tx.commit().await?;
    Ok(payload)
}

pub async fn get_project_commit_state(
    pool: &sqlx::PgPool,
    project_id: &str,
    local_commit_id: Option<&str>,
) -> Result<CommitStateResponse, ServerError> {
    let mut tx = pool.begin().await?;
    memory::service::project_org_id(&mut tx, project_id).await?;
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

pub async fn list_org_commits(
    pool: &sqlx::PgPool,
    org_id: &str,
) -> Result<CommitListResponse, ServerError> {
    repository::list_org_commits(pool, org_id).await
}

pub async fn get_org_commit_state(
    pool: &sqlx::PgPool,
    org_id: &str,
    local_commit_id: Option<&str>,
) -> Result<CommitStateResponse, ServerError> {
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
pub(crate) use super::repository::advance_org_ref;
pub(crate) use super::repository::advance_project_ref;
pub(crate) use super::repository::create_org_commit;
pub(crate) use super::repository::create_project_commit;
pub(crate) use super::repository::current_org_ref;
pub(crate) use super::repository::current_project_ref;
pub(crate) use super::repository::load_org_ref;
pub(crate) use super::repository::load_project_ref;
pub(crate) use super::repository::lock_org_ref_for_project_projection;
pub(crate) use super::repository::store_blob;
pub(crate) use super::repository::validate_org_commit;
pub(crate) use super::repository::validate_project_commit;
