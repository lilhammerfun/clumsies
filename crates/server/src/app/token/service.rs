//! Application operations and transaction coordination for token resources.

use super::repository;
use crate::app::audit_event;
use crate::app::auth::AuthPrincipal;
use crate::app::token::dto::AccessTokenListResponse;
use crate::dto::DeleteResult;
use crate::error::ServerError;
use crate::pagination::admin_page;

pub async fn list_admin_tokens(
    pool: &sqlx::PgPool,
    org_id: &str,
    offset: i64,
    limit: i64,
) -> Result<AccessTokenListResponse, ServerError> {
    let items = repository::list_access_tokens(pool, org_id, offset, limit + 1).await?;
    let (items, page_info) = admin_page(items, offset, limit);
    Ok(AccessTokenListResponse { items, page_info })
}

pub async fn delete_admin_token(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    token_id: &str,
) -> Result<DeleteResult, ServerError> {
    let mut tx = pool.begin().await?;
    if !repository::access_token_exists(&mut tx, &principal.org_id, token_id).await? {
        return Err(ServerError::not_found("access_token", token_id));
    }
    repository::revoke_access_token(&mut tx, &principal.org_id, token_id).await?;
    audit_event::service::insert_audit_event(
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
