//! Application operations and transaction coordination for memory resources.

use super::model::{OrgResourceImpact, etag};
use super::repository;
use super::repository::{list_resource_rows, load_resource_detail_row, load_target_resource};
use crate::app::commit;
use crate::app::commit::model::PendingTreeEntry;
use crate::app::commit::service::store_blob;
use crate::app::draft::dto::{DraftOperationAction, DraftOperationInput};
use crate::app::memory::dto::{
    MemoryDetail, MemoryExport, MemoryListResponse, MemoryMeta, ProjectOrgSelection,
    ReplaceProjectOrgSelectionRequest,
};
use crate::app::memory::model::validate_resource_path;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use sqlx::{PgPool, Postgres, Row, Transaction};

pub async fn export_memory_state(
    pool: &sqlx::PgPool,
    org_id: &str,
) -> Result<MemoryExport, ServerError> {
    repository::export_memory_state(pool, org_id).await
}

pub async fn create_org_context(
    pool: &sqlx::PgPool,
    org_id: &str,
    path: &str,
    body: &str,
) -> Result<String, ServerError> {
    let resource_id = prefixed_id("mem");
    let mut tx = pool.begin().await?;
    repository::insert_org_context(&mut tx, &resource_id, org_id, path, body).await?;
    let parent_commit_id = commit::service::current_org_ref(&mut tx, org_id).await?;
    let commit_id =
        commit::service::create_org_commit(&mut tx, org_id, parent_commit_id.as_deref()).await?;
    commit::service::advance_org_ref(&mut tx, org_id, &commit_id).await?;
    tx.commit().await?;
    Ok(resource_id)
}

pub async fn select_org_resource_for_project(
    pool: &sqlx::PgPool,
    project_id: &str,
    resource_id: &str,
) -> Result<(), ServerError> {
    let mut tx = pool.begin().await?;
    let org_id = repository::project_org_id(&mut tx, project_id).await?;
    commit::service::lock_org_ref_for_project_projection(&mut tx, &org_id).await?;
    let parent_commit_id = commit::service::current_project_ref(&mut tx, project_id).await?;
    if !repository::org_resource_exists(&mut tx, &org_id, resource_id).await? {
        return Err(ServerError::not_found("org_resource", resource_id));
    }
    let current_revision =
        repository::current_project_org_selection_revision(&mut tx, project_id).await?;
    let next_revision = current_revision + 1;
    repository::upsert_project_org_selection(&mut tx, project_id, resource_id, next_revision)
        .await?;
    repository::validate_project_effective_memory(&mut tx, project_id, &org_id).await?;
    repository::update_project_org_selection_revision(&mut tx, project_id, next_revision).await?;
    let commit_id =
        commit::service::create_project_commit(&mut tx, project_id, parent_commit_id.as_deref())
            .await?;
    commit::service::advance_project_ref(&mut tx, project_id, &commit_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn replace_project_org_selection(
    pool: &sqlx::PgPool,
    project_id: &str,
    expected_revision: i64,
    request: ReplaceProjectOrgSelectionRequest,
) -> Result<ProjectOrgSelection, ServerError> {
    let mut tx = pool.begin().await?;
    lock_org_draft_selection_coordination_for_project(&mut tx, project_id).await?;
    let org_id = repository::project_org_id(&mut tx, project_id).await?;
    commit::service::lock_org_ref_for_project_projection(&mut tx, &org_id).await?;
    let parent_commit_id = commit::service::current_project_ref(&mut tx, project_id).await?;
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
        commit::service::create_project_commit(&mut tx, project_id, parent_commit_id.as_deref())
            .await?;
    commit::service::advance_project_ref(&mut tx, project_id, &commit_id).await?;
    let selection = repository::load_project_org_selection(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(selection)
}

pub async fn list_org_memories(
    pool: &sqlx::PgPool,
    org_id: &str,
) -> Result<MemoryListResponse, ServerError> {
    Ok(MemoryListResponse {
        items: list_memory_meta(pool, "org", Some(org_id), None).await?,
        page_info: crate::pagination::page_info(),
    })
}

pub async fn list_project_memories(
    pool: &sqlx::PgPool,
    project_id: &str,
) -> Result<MemoryListResponse, ServerError> {
    Ok(MemoryListResponse {
        items: list_memory_meta(pool, "project", None, Some(project_id)).await?,
        page_info: crate::pagination::page_info(),
    })
}

pub async fn get_org_memory(
    pool: &sqlx::PgPool,
    org_id: &str,
    memory_id: &str,
) -> Result<MemoryDetail, ServerError> {
    let mut tx = pool.begin().await?;
    let detail = load_memory_detail(&mut tx, memory_id, "org", Some(org_id), None).await?;
    tx.commit().await?;
    Ok(detail)
}

pub async fn get_project_memory(
    pool: &sqlx::PgPool,
    project_id: &str,
    memory_id: &str,
) -> Result<MemoryDetail, ServerError> {
    let mut tx = pool.begin().await?;
    let detail = load_memory_detail(&mut tx, memory_id, "project", None, Some(project_id)).await?;
    tx.commit().await?;
    Ok(detail)
}

pub async fn get_project_org_selection(
    pool: &sqlx::PgPool,
    project_id: &str,
) -> Result<ProjectOrgSelection, ServerError> {
    let mut tx = pool.begin().await?;
    let selection = repository::load_project_org_selection(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(selection)
}

pub(crate) async fn lock_org_draft_selection_coordination_for_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<(), ServerError> {
    let org_id = project_org_id(tx, project_id).await?;
    lock_org_draft_selection_coordination(tx, &org_id).await
}

pub(crate) async fn list_memory_meta(
    pool: &PgPool,
    scope: &str,
    org_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<Vec<MemoryMeta>, ServerError> {
    let rows = list_resource_rows(pool, scope, org_id, project_id).await?;
    rows.iter().map(memory_meta_from_row).collect()
}

pub(crate) async fn load_memory_detail(
    tx: &mut Transaction<'_, Postgres>,
    memory_id: &str,
    scope: &str,
    org_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<MemoryDetail, ServerError> {
    let row = load_resource_detail_row(tx, memory_id, scope, org_id, project_id).await?;
    let memory = memory_meta_from_row(&row)?;
    Ok(MemoryDetail {
        content: row.try_get("body")?,
        etag: etag(row.try_get("revision")?),
        memory,
    })
}

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

pub(crate) async fn pending_resource_entry(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
    scope: &str,
    project_id: Option<&str>,
    source: &str,
) -> Result<PendingTreeEntry, ServerError> {
    let resource_id: String = row.try_get("resource_id")?;
    let body: String = row.try_get("body")?;
    let description: String = row.try_get("description")?;
    let resource_kind: String = row.try_get("resource_kind")?;
    let path: String = row.try_get("path")?;
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
pub(crate) use super::repository::apply_resource_operation;
pub(crate) use super::repository::load_project_org_selection;
pub(crate) use super::repository::lock_org_draft_selection_coordination;
pub(crate) use super::repository::memory_meta_from_row;
pub(crate) use super::repository::project_org_id;
pub(crate) use super::repository::refresh_projects_for_org_resource_changes;
pub(crate) use super::repository::select_created_org_resources_for_project;
pub(crate) use super::repository::user_ref;
pub(crate) use super::repository::validate_project_effective_memory;
