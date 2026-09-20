//! SQL queries and persistence for bundle resources.

use super::model::PersonalBundleSnapshot;
use crate::app::bundle::dto::{
    PersonalBundleListResponse, PersonalBundleMeta, PersonalBundleRequest,
};
use crate::error::ServerError;
use crate::pagination::page_info;
use sqlx::{PgPool, Postgres, Row, Transaction};

/// Persist a bundle's deduplicated active resource selection after checking ownership scope.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn insert_bundle_items(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
    org_id: &str,
    resource_ids: &[String],
) -> Result<(), ServerError> {
    for (position, resource_id) in resource_ids.iter().enumerate() {
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
            "INSERT INTO personal_bundle_items (
                bundle_id, resource_id, position
             )
             VALUES ($1, $2, $3)",
        )
        .bind(bundle_id)
        .bind(resource_id)
        .bind(position as i32)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Replace selected resources when supplied, preserving the current set when omitted.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn replace_bundle_items_if_present(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
    org_id: &str,
    resource_ids: Option<Vec<String>>,
) -> Result<(), ServerError> {
    if let Some(resource_ids) = resource_ids {
        sqlx::query("DELETE FROM personal_bundle_items WHERE bundle_id = $1")
            .bind(bundle_id)
            .execute(&mut **tx)
            .await?;
        insert_bundle_items(tx, bundle_id, org_id, &resource_ids).await?;
    }
    Ok(())
}

/// Read owner-visible bundle metadata and its active selected-resource count.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_personal_bundle_meta(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
) -> Result<PersonalBundleMeta, ServerError> {
    let bundle_row = sqlx::query(
        "SELECT
            b.bundle_id, b.owner_user_id, b.name, b.description, b.revision,
            b.created_at, b.updated_at,
            count(i.resource_id) AS resource_count
         FROM personal_bundles b
         LEFT JOIN personal_bundle_items i ON i.bundle_id = b.bundle_id
         WHERE b.bundle_id = $1
         GROUP BY b.bundle_id",
    )
    .bind(bundle_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("bundle", bundle_id))?;

    personal_bundle_meta_from_row(&bundle_row)
}

/// Decode bundle metadata and its owner identity from one joined row.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) fn personal_bundle_meta_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<PersonalBundleMeta, ServerError> {
    Ok(PersonalBundleMeta {
        bundle_id: row.try_get("bundle_id")?,
        owner_user_id: row.try_get("owner_user_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        resource_count: row.try_get::<i64, _>("resource_count")?,
        revision: row.try_get("revision")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Hide bundles outside the requesting owner's organization and user identity.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn ensure_bundle_owner(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
    owner_user_id: &str,
) -> Result<(), ServerError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM personal_bundles
            WHERE bundle_id = $1 AND owner_user_id = $2
         )",
    )
    .bind(bundle_id)
    .bind(owner_user_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ServerError::not_found("bundle", bundle_id))
    }
}

/// Insert initial owned bundle metadata and its first concurrency revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn insert_personal_bundle(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
    owner_user_id: &str,
    request: &PersonalBundleRequest,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO personal_bundles (
            bundle_id, owner_user_id, name, description, revision
         )
         VALUES ($1, $2, $3, $4, 1)",
    )
    .bind(bundle_id)
    .bind(owner_user_id)
    .bind(&request.name)
    .bind(request.description.as_deref().unwrap_or_default())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Lock an owned bundle and load metadata needed for an update.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn lock_personal_bundle(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
    owner_user_id: &str,
) -> Result<PersonalBundleSnapshot, ServerError> {
    let row = sqlx::query(
        "SELECT name, description, revision
         FROM personal_bundles
         WHERE bundle_id = $1 AND owner_user_id = $2
         FOR UPDATE",
    )
    .bind(bundle_id)
    .bind(owner_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("bundle", bundle_id))?;
    Ok(PersonalBundleSnapshot {
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        revision: row.try_get("revision")?,
    })
}

/// Persist replacement bundle metadata and advance its concurrency revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn update_personal_bundle_metadata(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
    name: &str,
    description: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE personal_bundles
         SET name = $2, description = $3, revision = revision + 1, updated_at = now()
         WHERE bundle_id = $1",
    )
    .bind(bundle_id)
    .bind(name)
    .bind(description)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Lock an owned bundle and read its revision before deletion.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn lock_personal_bundle_revision(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
    owner_user_id: &str,
) -> Result<i64, ServerError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT revision
         FROM personal_bundles
         WHERE bundle_id = $1 AND owner_user_id = $2
         FOR UPDATE",
    )
    .bind(bundle_id)
    .bind(owner_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("bundle", bundle_id))
}

/// Delete an owned collection after the service has checked its revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn delete_personal_bundle(
    tx: &mut Transaction<'_, Postgres>,
    bundle_id: &str,
) -> Result<(), ServerError> {
    sqlx::query("DELETE FROM personal_bundles WHERE bundle_id = $1")
        .bind(bundle_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Read the owner's collections and their active selected-resource counts.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_personal_bundles(
    pool: &PgPool,
    owner_user_id: &str,
) -> Result<PersonalBundleListResponse, ServerError> {
    let rows = sqlx::query(
        "SELECT
            b.bundle_id, b.owner_user_id, b.name, b.description, b.revision,
            b.created_at, b.updated_at,
            count(i.resource_id) AS resource_count
         FROM personal_bundles b
         LEFT JOIN personal_bundle_items i ON i.bundle_id = b.bundle_id
         WHERE b.owner_user_id = $1
         GROUP BY b.bundle_id
         ORDER BY b.updated_at DESC
         LIMIT 50",
    )
    .bind(owner_user_id)
    .fetch_all(pool)
    .await?;

    Ok(PersonalBundleListResponse {
        items: rows
            .iter()
            .map(personal_bundle_meta_from_row)
            .collect::<Result<Vec<_>, _>>()?,
        page_info: page_info(),
    })
}
