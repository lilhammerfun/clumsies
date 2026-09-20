//! Application operations and transaction coordination for bundle resources.

use super::repository;
use crate::app::bundle::dto::{
    PersonalBundleDetail, PersonalBundleListResponse, PersonalBundleRequest,
    PersonalBundleUpdateRequest,
};
use crate::app::memory;
use crate::dto::DeleteResult;
use crate::error::ServerError;
use crate::identity::prefixed_id;

/// Create an owned Memory collection and its validated resource selection atomically.
///
/// # Errors
/// Rejects missing owners and resources outside the organization, or propagates database
/// failures; the collection and its selections are persisted together.
pub async fn create_personal_bundle(
    pool: &sqlx::PgPool,
    owner_user_id: &str,
    org_id: &str,
    request: PersonalBundleRequest,
) -> Result<PersonalBundleDetail, ServerError> {
    let mut tx = pool.begin().await?;
    memory::user_ref(&mut tx, owner_user_id).await?;
    let bundle_id = prefixed_id("bdl");
    repository::insert_personal_bundle(&mut tx, &bundle_id, owner_user_id, &request).await?;
    repository::insert_bundle_items(&mut tx, &bundle_id, org_id, &request.resource_ids).await?;
    tx.commit().await?;
    get_personal_bundle(pool, owner_user_id, &bundle_id).await
}

/// Return collections belonging to the authenticated owner.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_personal_bundles(
    pool: &sqlx::PgPool,
    owner_user_id: &str,
) -> Result<PersonalBundleListResponse, ServerError> {
    repository::list_personal_bundles(pool, owner_user_id).await
}

/// Require collection ownership before assembling its active selected memories.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn get_personal_bundle(
    pool: &sqlx::PgPool,
    owner_user_id: &str,
    bundle_id: &str,
) -> Result<PersonalBundleDetail, ServerError> {
    let mut tx = pool.begin().await?;
    repository::ensure_bundle_owner(&mut tx, bundle_id, owner_user_id).await?;
    let detail = load_personal_bundle_detail(&mut tx, bundle_id).await?;
    tx.commit().await?;
    Ok(detail)
}

/// Apply owner-authorized metadata and selection changes at the expected revision.
///
/// # Errors
/// Hides collections owned by another user, rejects stale revisions or invalid resource
/// selections, and propagates persistence failures.
pub async fn update_personal_bundle(
    pool: &sqlx::PgPool,
    owner_user_id: &str,
    org_id: &str,
    bundle_id: &str,
    expected_revision: i64,
    request: PersonalBundleUpdateRequest,
) -> Result<PersonalBundleDetail, ServerError> {
    let mut tx = pool.begin().await?;
    let current = repository::lock_personal_bundle(&mut tx, bundle_id, owner_user_id).await?;
    if current.revision != expected_revision {
        return Err(ServerError::version_conflict(
            "bundle",
            expected_revision,
            current.revision,
        ));
    }
    let name = request.name.unwrap_or(current.name);
    let description = request.description.unwrap_or(current.description);
    repository::update_personal_bundle_metadata(&mut tx, bundle_id, &name, &description).await?;
    repository::replace_bundle_items_if_present(&mut tx, bundle_id, org_id, request.resource_ids)
        .await?;
    let detail = load_personal_bundle_detail(&mut tx, bundle_id).await?;
    tx.commit().await?;
    Ok(detail)
}

/// Delete an owned collection only when its revision matches the caller's precondition.
///
/// # Errors
/// Hides collections owned by another user, rejects stale revisions, and propagates persistence
/// failures.
pub async fn delete_personal_bundle(
    pool: &sqlx::PgPool,
    owner_user_id: &str,
    bundle_id: &str,
    expected_revision: i64,
) -> Result<DeleteResult, ServerError> {
    let mut tx = pool.begin().await?;
    let current_revision =
        repository::lock_personal_bundle_revision(&mut tx, bundle_id, owner_user_id).await?;
    if current_revision != expected_revision {
        return Err(ServerError::version_conflict(
            "bundle",
            expected_revision,
            current_revision,
        ));
    }
    repository::delete_personal_bundle(&mut tx, bundle_id).await?;
    tx.commit().await?;
    Ok(DeleteResult {
        deleted: true,
        id: bundle_id.to_owned(),
    })
}

/// Assemble collection metadata and active Memory selections through their resource interfaces.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
async fn load_personal_bundle_detail(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    bundle_id: &str,
) -> Result<PersonalBundleDetail, ServerError> {
    let bundle = repository::load_personal_bundle_meta(tx, bundle_id).await?;
    let memories = memory::list_bundle_memories(tx, bundle_id).await?;
    Ok(PersonalBundleDetail {
        etag: crate::app::memory::model::etag(bundle.revision),
        bundle,
        memories,
    })
}
