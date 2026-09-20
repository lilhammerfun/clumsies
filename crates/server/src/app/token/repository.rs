//! SQL queries and persistence for token resources.

use super::model::access_token_kind;
use crate::app::token::dto::AccessTokenMeta;
use crate::error::ServerError;
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;

/// Read non-secret organization credential metadata with filtering before pagination.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_access_tokens(
    pool: &PgPool,
    org_id: &str,
    offset: i64,
    limit: i64,
) -> Result<Vec<AccessTokenMeta>, ServerError> {
    let rows = sqlx::query(
        "SELECT t.token_id, t.user_id, t.kind, t.revoked_at, t.expires_at, t.created_at
         FROM access_tokens t
         JOIN auth_sessions s ON s.session_id = t.session_id
         WHERE s.org_id = $1
         ORDER BY t.created_at DESC, t.token_id
         LIMIT $2 OFFSET $3",
    )
    .bind(org_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(access_token_meta_from_row).collect()
}

/// Check organization ownership of a credential without exposing its secret.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn access_token_exists(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    token_id: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM access_tokens t
            JOIN auth_sessions s ON s.session_id = t.session_id
            WHERE t.token_id = $1 AND s.org_id = $2
         )",
    )
    .bind(token_id)
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Invalidate a credential inside the caller's administrative transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn revoke_access_token(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    token_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE access_tokens t SET revoked_at = now()
         FROM auth_sessions s
         WHERE t.session_id = s.session_id AND t.token_id = $1 AND s.org_id = $2",
    )
    .bind(token_id)
    .bind(org_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Decode safe credential metadata and its public owner identity.
///
/// # Errors
/// Propagates database access and row-decoding failures.
fn access_token_meta_from_row(row: &sqlx::postgres::PgRow) -> Result<AccessTokenMeta, ServerError> {
    Ok(AccessTokenMeta {
        token_id: row.try_get("token_id")?,
        user_id: row.try_get("user_id")?,
        kind: access_token_kind(row.try_get::<String, _>("kind")?.as_str())?,
        revoked: row
            .try_get::<Option<OffsetDateTime>, _>("revoked_at")?
            .is_some(),
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
    })
}
