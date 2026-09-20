//! Personal receipt operations; membership is checked in the same query as the read or write.

use super::dto::{InboxListResponse, UpdateInboxRequest};
use super::repository;
use crate::app::auth::AuthPrincipal;
use crate::error::ServerError;
use sqlx::PgPool;

/// List a bounded page of the principal's currently accessible notifications.
///
/// # Errors
/// Rejects invalid page sizes and propagates database failures.
pub async fn list(
    pool: &PgPool,
    principal: &AuthPrincipal,
    cursor: Option<&str>,
    limit: Option<i64>,
) -> Result<InboxListResponse, ServerError> {
    let limit = limit.unwrap_or(100);
    if !(1..=200).contains(&limit) {
        return Err(ServerError::InvalidRequest(
            "inbox limit must be between 1 and 200".into(),
        ));
    }
    repository::list(pool, principal, cursor, limit).await
}

/// Update only the principal's receipt after checking the displayed revision and current access.
///
/// # Errors
/// Rejects invalid/future revisions and inaccessible subjects; propagates database failures.
pub async fn update(
    pool: &PgPool,
    principal: &AuthPrincipal,
    id: &str,
    request: UpdateInboxRequest,
) -> Result<(), ServerError> {
    if request.version < 1 {
        return Err(ServerError::InvalidRequest(
            "inbox version must be positive".into(),
        ));
    }
    repository::update(pool, principal, id, request).await
}
