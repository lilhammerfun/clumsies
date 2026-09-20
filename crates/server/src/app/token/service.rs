//! Application operations and transaction coordination for token resources.

use super::repository;
use crate::app::audit_event;
use crate::app::auth::AuthPrincipal;
use crate::app::token::dto::AccessTokenListResponse;
use crate::dto::DeleteResult;
use crate::error::ServerError;
use crate::pagination::admin_page;

/// Return non-secret credential metadata after checking organization administration privileges.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_admin_tokens(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    offset: i64,
    limit: i64,
) -> Result<AccessTokenListResponse, ServerError> {
    let org_id = &principal.org_id;
    principal.require_org_admin()?;

    let items = repository::list_access_tokens(pool, org_id, offset, limit + 1).await?;
    let (items, page_info) = admin_page(items, offset, limit);
    Ok(AccessTokenListResponse { items, page_info })
}

/// Revoke an organization credential and record the actor in the same transaction.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn delete_admin_token(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    token_id: &str,
) -> Result<DeleteResult, ServerError> {
    principal.require_org_admin()?;

    let mut tx = pool.begin().await?;
    if !repository::access_token_exists(&mut tx, &principal.org_id, token_id).await? {
        return Err(ServerError::not_found("access_token", token_id));
    }
    repository::revoke_access_token(&mut tx, &principal.org_id, token_id).await?;
    audit_event::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.token_revoked",
        "access_token",
        Some(token_id),
    )
    .await?;
    tx.commit().await?;
    Ok(DeleteResult {
        deleted: true,
        id: token_id.to_owned(),
    })
}
