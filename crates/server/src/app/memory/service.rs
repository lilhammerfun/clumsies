//! Memory use cases and transaction coordination.

use super::repository::{
    current_project_org_selection_revision, update_project_org_selection_revision,
};
use crate::app::auth::AuthPrincipal;
use crate::app::commit::model::PendingTreeEntry;
use crate::app::commit::{
    advance_project_ref, create_project_commit, current_project_ref, store_blob,
};
use crate::app::draft::dto::{DraftOperationAction, DraftOperationInput};
use crate::app::memory::dto::{
    MemoryDetail, MemoryExport, MemoryListResponse, ProjectOrgSelection,
    ReplaceProjectOrgSelectionRequest, ResourceScope,
};
use crate::app::memory::model::{
    OrgResourceImpact, content_hash, insert_materialization_path, materialization_output_path,
    name_from_path, prepare_resource_content, validate_resource_path,
};
use crate::app::memory::repository;
use crate::app::memory::repository::{list_memory_meta, load_memory_detail, load_target_resource};
use crate::app::memory::repository::{lock_org_draft_selection_coordination, project_org_id};
use crate::app::{commit, project};
use crate::error::ServerError;
use crate::identity::prefixed_id;
use sqlx::{Postgres, Transaction};
use std::collections::BTreeMap;

/// Export verifiable organization Memory state after checking administrative privileges.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn export_memory_state(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
) -> Result<MemoryExport, ServerError> {
    let org_id = &principal.org_id;
    principal.require_org_admin()?;

    repository::export_memory_state(pool, org_id).await
}

/// Persist initial organization content and advance its authoritative snapshot atomically.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn create_org_context(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    path: &str,
    body: &str,
) -> Result<String, ServerError> {
    let org_id = &principal.org_id;
    principal.require_org_admin()?;

    let resource_id = prefixed_id("mem");
    let mut tx = pool.begin().await?;
    repository::insert_org_context(&mut tx, &resource_id, org_id, path, body).await?;
    let parent_commit_id = commit::current_org_ref(&mut tx, org_id).await?;
    let commit_id = commit::create_org_commit(&mut tx, org_id, parent_commit_id.as_deref()).await?;
    commit::advance_org_ref(&mut tx, org_id, &commit_id).await?;
    tx.commit().await?;
    Ok(resource_id)
}

/// Validate administrator access, select one organization resource, and refresh the project
/// snapshot.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
pub async fn select_org_resource_for_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    resource_id: &str,
) -> Result<(), ServerError> {
    project::ensure_project_admin(pool, principal, project_id).await?;
    project::ensure_project_member(pool, principal, project_id).await?;

    let mut tx = pool.begin().await?;
    let org_id = repository::project_org_id(&mut tx, project_id).await?;
    commit::lock_org_ref_for_project_projection(&mut tx, &org_id).await?;
    let parent_commit_id = commit::current_project_ref(&mut tx, project_id).await?;
    if !repository::org_resource_exists(&mut tx, &org_id, resource_id).await? {
        return Err(ServerError::not_found("org_resource", resource_id));
    }
    let current_revision =
        repository::current_project_org_selection_revision(&mut tx, project_id).await?;
    let next_revision = current_revision + 1;
    repository::upsert_project_org_selection(&mut tx, project_id, resource_id, next_revision)
        .await?;
    validate_project_effective_memory(&mut tx, project_id, &org_id).await?;
    repository::update_project_org_selection_revision(&mut tx, project_id, next_revision).await?;
    let commit_id =
        commit::create_project_commit(&mut tx, project_id, parent_commit_id.as_deref()).await?;
    commit::advance_project_ref(&mut tx, project_id, &commit_id).await?;
    tx.commit().await?;
    Ok(())
}

/// Replace selected organization resources at the expected revision while coordinating active
/// drafts.
///
/// # Errors
/// Rejects insufficient project permissions, stale selection revisions, invalid or foreign
/// resources, active proposals targeting removed selections, and materialization path conflicts.
/// The selection and project reference are committed together.
pub async fn replace_project_org_selection(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    expected_revision: i64,
    request: ReplaceProjectOrgSelectionRequest,
) -> Result<ProjectOrgSelection, ServerError> {
    project::ensure_project_admin(pool, principal, project_id).await?;
    project::ensure_project_member(pool, principal, project_id).await?;

    let mut tx = pool.begin().await?;
    lock_org_draft_selection_coordination_for_project(&mut tx, project_id).await?;
    let org_id = repository::project_org_id(&mut tx, project_id).await?;
    commit::lock_org_ref_for_project_projection(&mut tx, &org_id).await?;
    let parent_commit_id = commit::current_project_ref(&mut tx, project_id).await?;
    let current_revision =
        repository::current_project_org_selection_revision(&mut tx, project_id).await?;
    if current_revision != expected_revision {
        return Err(ServerError::version_conflict(
            "project_org_selection",
            expected_revision,
            current_revision,
        ));
    }
    repository::ensure_removed_org_resources_have_no_active_drafts(
        &mut tx,
        project_id,
        &request.resource_ids,
    )
    .await?;
    let next_revision = current_revision + 1;
    repository::delete_project_org_selections(&mut tx, project_id).await?;
    repository::insert_project_org_selection_items(
        &mut tx,
        project_id,
        &org_id,
        next_revision,
        &request.resource_ids,
    )
    .await?;
    repository::update_project_org_selection_revision(&mut tx, project_id, next_revision).await?;
    let commit_id =
        commit::create_project_commit(&mut tx, project_id, parent_commit_id.as_deref()).await?;
    commit::advance_project_ref(&mut tx, project_id, &commit_id).await?;
    let selection = repository::load_project_org_selection(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(selection)
}

/// Return active resources within the authenticated identity's organization.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_org_memories(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
) -> Result<MemoryListResponse, ServerError> {
    let org_id = &principal.org_id;

    Ok(MemoryListResponse {
        items: list_memory_meta(pool, "org", Some(org_id), None).await?,
        page_info: crate::pagination::page_info(),
    })
}

/// Return active project resources after checking project membership.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_project_memories(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
) -> Result<MemoryListResponse, ServerError> {
    project::ensure_project_member(pool, principal, project_id).await?;

    Ok(MemoryListResponse {
        items: list_memory_meta(pool, "project", None, Some(project_id)).await?,
        page_info: crate::pagination::page_info(),
    })
}

/// Return one active resource within the authenticated identity's organization.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_org_memory(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    memory_id: &str,
) -> Result<MemoryDetail, ServerError> {
    let org_id = &principal.org_id;

    let mut tx = pool.begin().await?;
    let detail = load_memory_detail(&mut tx, memory_id, "org", Some(org_id), None).await?;
    tx.commit().await?;
    Ok(detail)
}

/// Return project resource content after enforcing membership and ownership scope.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_project_memory(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    memory_id: &str,
) -> Result<MemoryDetail, ServerError> {
    project::ensure_project_member(pool, principal, project_id).await?;

    let mut tx = pool.begin().await?;
    let detail = load_memory_detail(&mut tx, memory_id, "project", None, Some(project_id)).await?;
    tx.commit().await?;
    Ok(detail)
}

/// Return a member-visible project's selected organization resources and selection revision.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_project_org_selection(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
) -> Result<ProjectOrgSelection, ServerError> {
    project::ensure_project_member(pool, principal, project_id).await?;

    let mut tx = pool.begin().await?;
    let selection = repository::load_project_org_selection(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(selection)
}

/// Serialize organization proposal mutations with selection changes for the project's
/// organization.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub(crate) async fn lock_org_draft_selection_coordination_for_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<(), ServerError> {
    let org_id = project_org_id(tx, project_id).await?;
    lock_org_draft_selection_coordination(tx, &org_id).await
}

/// Collect changed and deleted resource identities needed to refresh dependent project snapshots.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub(crate) async fn resolve_org_resource_impact(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    operations: &[DraftOperationInput],
) -> Result<OrgResourceImpact, ServerError> {
    let mut impact = OrgResourceImpact::default();
    for operation in operations {
        if operation.action == DraftOperationAction::Create {
            continue;
        }
        let target = load_target_resource(tx, org_id, None, &operation.resource).await?;
        impact.resource_ids.insert(target.resource_id.clone());
        if operation.action == DraftOperationAction::Delete {
            impact.deleted_resource_ids.insert(target.resource_id);
        }
    }
    Ok(impact)
}

/// Validate a Memory path and store its content-addressed blob for a pending snapshot entry.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
pub(crate) async fn pending_resource_entry(
    tx: &mut Transaction<'_, Postgres>,
    row: &super::model::CommitResource,
    scope: &str,
    project_id: Option<&str>,
    source: &str,
) -> Result<PendingTreeEntry, ServerError> {
    let resource_id: String = row.resource_id.clone();
    let body: String = row.body.clone();
    let description: String = row.description.clone();
    let resource_kind: String = row.resource_kind.clone();
    let path: String = row.path.clone();
    validate_resource_path(&path)?;
    match resource_kind.as_str() {
        "memory" => {}
        other => {
            return Err(ServerError::InvalidRequest(format!(
                "unknown resource kind while creating Commit: {other}"
            )));
        }
    }
    Ok(PendingTreeEntry {
        item_id: resource_id,
        resource_kind,
        scope: scope.to_owned(),
        project_id: project_id.map(ToOwned::to_owned),
        path: Some(path),
        blob_id: store_blob(tx, &body).await?,
        source: source.to_owned(),
        description,
    })
}

// Transaction participants share the caller-owned transaction; only the outer service commits.

/// Validate and apply one create, edit, rename, or archive action within the caller's
/// transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
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
            repository::insert_memory(
                tx,
                repository::NewMemory {
                    resource_id: &resource_id,
                    org_id: &org_id,
                    project_id: resource_project_id,
                    scope: scope.as_str(),
                    path,
                    name: &prepared.name,
                    content_hash: content_hash(&prepared.body),
                    body: &prepared.body,
                },
            )
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
            repository::update_memory_content(
                tx,
                &resource.resource_id,
                &prepared.name,
                &prepared.body,
                content_hash(&prepared.body),
            )
            .await?;
        }
        DraftOperationAction::Rename => {
            let resource =
                load_target_resource(tx, &org_id, resource_project_id, &operation.resource).await?;
            let new_path = operation.new_path.as_ref().ok_or_else(|| {
                ServerError::InvalidRequest("rename operation requires new_path".to_owned())
            })?;
            repository::rename_memory(
                tx,
                &resource.resource_id,
                new_path,
                name_from_path(new_path),
            )
            .await?;
        }
        DraftOperationAction::Delete => {
            let resource =
                load_target_resource(tx, &org_id, resource_project_id, &operation.resource).await?;
            repository::archive_memory(tx, &resource.resource_id).await?;
        }
    }
    Ok(None)
}

/// Include newly published organization resources in the proposing project's effective snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
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
        repository::select_created_memory(tx, project_id, resource_id, next_revision).await?;
    }
    validate_project_effective_memory(tx, project_id, org_id).await?;
    update_project_org_selection_revision(tx, project_id, next_revision).await?;
    let commit_id = create_project_commit(tx, project_id, parent_commit_id.as_deref()).await?;
    advance_project_ref(tx, project_id, &commit_id).await?;
    Ok(())
}

/// Refresh every affected project head and remove selections of deleted organization resources.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub(crate) async fn refresh_projects_for_org_resource_changes(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    impact: &OrgResourceImpact,
) -> Result<(), ServerError> {
    if impact.resource_ids.is_empty() {
        return Ok(());
    }

    let resource_ids = impact.resource_ids.iter().cloned().collect::<Vec<_>>();
    let rows = repository::list_selecting_projects(tx, org_id, &resource_ids).await?;
    let deleted_resource_ids = impact
        .deleted_resource_ids
        .iter()
        .cloned()
        .collect::<Vec<_>>();

    for row in rows {
        let project_id: String = row.project_id.clone();
        let parent_commit_id = current_project_ref(tx, &project_id).await?;
        if !deleted_resource_ids.is_empty() {
            let deleted =
                repository::remove_deleted_selections(tx, &project_id, &deleted_resource_ids)
                    .await?;
            if deleted > 0 {
                let revision = current_project_org_selection_revision(tx, &project_id).await?;
                update_project_org_selection_revision(tx, &project_id, revision + 1).await?;
            }
        }
        let commit_id = create_project_commit(tx, &project_id, parent_commit_id.as_deref()).await?;
        advance_project_ref(tx, &project_id, &commit_id).await?;
    }
    Ok(())
}

/// Reject cross-organization selections and colliding or unsafe materialized paths.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
pub(crate) async fn validate_project_effective_memory(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
) -> Result<(), ServerError> {
    let cross_org_resource = repository::find_cross_org_selection(tx, project_id, org_id).await?;
    if let Some(resource_id) = cross_org_resource {
        return Err(ServerError::InvalidRequest(format!(
            "project cannot select a resource from another organization: {resource_id}"
        )));
    }

    let rows = repository::list_effective_paths(tx, project_id, org_id).await?;
    let mut output_paths = BTreeMap::new();
    for row in rows {
        let resource_id: String = row.resource_id.clone();
        let path: String = row.path.clone();
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

/// Returns authorized, server-aggregated published memory statistics.
///
/// # Errors
/// Rejects inaccessible projects, invalid periods/time zones and database failures.
pub(super) async fn memory_statistics(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: Option<&str>,
    query: super::dto::MemoryStatisticsQuery,
) -> Result<super::dto::MemoryStatistics, ServerError> {
    if let Some(project_id) = project_id {
        project::ensure_project_member(pool, principal, project_id).await?;
    }
    if !matches!(query.days, 7 | 30 | 90) || query.time_zone.len() > 100 {
        return Err(ServerError::InvalidRequest(
            "Expected 7, 30 or 90 days and an IANA time zone".into(),
        ));
    }
    super::statistics::load(pool, principal, project_id, query).await
}
