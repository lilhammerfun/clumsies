//! Application operations and transaction coordination for draft resources.

use super::model::state_hash;
use super::repository;
use super::repository::{resolve_org_draft_target_id, validate_org_draft_target_is_selected};
use crate::app::auth::AuthPrincipal;
use crate::app::commit::service::{current_org_ref, current_project_ref};
use crate::app::draft::dto::{
    CreateDraftRebaseRequest, CreateDraftReconciliationCandidateRequest, CreateDraftRequest,
    DraftDetail, DraftEventListResponse, DraftListResponse, DraftOperation, DraftOperationAction,
    DraftOperationBatchRequest, DraftOperationBatchResponse, DraftOperationInput,
    DraftRebaseResult, DraftReconciliationCandidate, DraftResourceRef, UpdateDraftRequest,
};
use crate::app::memory::dto::ResourceScope;
use crate::app::memory::service::{apply_resource_operation, project_org_id};
use crate::dto::DeleteResult;
use crate::error::ServerError;
use sqlx::{Postgres, Transaction};

pub async fn ensure_draft_owner(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    draft_id: &str,
) -> Result<(), ServerError> {
    if repository::draft_is_owned_by(pool, principal, draft_id).await? {
        Ok(())
    } else {
        Err(ServerError::not_found("draft", draft_id))
    }
}

pub async fn create_draft(
    pool: &sqlx::PgPool,
    author_user_id: &str,
    request: CreateDraftRequest,
) -> Result<DraftDetail, ServerError> {
    let mut tx = pool.begin().await?;
    let draft_id = repository::create_draft(&mut tx, author_user_id, request).await?;
    tx.commit().await?;
    get_draft(pool, &draft_id).await
}

pub async fn list_drafts(
    pool: &sqlx::PgPool,
    author_user_id: &str,
    project_id: Option<&str>,
) -> Result<DraftListResponse, ServerError> {
    repository::list_drafts(pool, author_user_id, project_id).await
}

pub async fn get_draft(pool: &sqlx::PgPool, draft_id: &str) -> Result<DraftDetail, ServerError> {
    let mut tx = pool.begin().await?;
    let detail = repository::load_draft_detail(&mut tx, draft_id).await?;
    tx.commit().await?;
    Ok(detail)
}

pub async fn append_draft_operation(
    pool: &sqlx::PgPool,
    draft_id: &str,
    expected_draft_version: i64,
    operation: DraftOperationInput,
) -> Result<DraftDetail, ServerError> {
    let mut tx = pool.begin().await?;
    repository::append_draft_operation_in_tx(
        &mut tx,
        draft_id,
        expected_draft_version,
        operation,
        None,
        false,
    )
    .await?;
    tx.commit().await?;
    get_draft(pool, draft_id).await
}

pub async fn create_draft_reconciliation_candidate(
    pool: &sqlx::PgPool,
    draft_id: &str,
    request: CreateDraftReconciliationCandidateRequest,
) -> Result<DraftReconciliationCandidate, ServerError> {
    let mut tx = pool.begin().await?;
    let candidate = repository::create_reconciliation_candidate_in_tx(
        &mut tx,
        draft_id,
        request.expected_draft_version,
    )
    .await?;
    tx.commit().await?;
    Ok(candidate)
}

pub async fn get_draft_reconciliation_candidate(
    pool: &sqlx::PgPool,
    draft_id: &str,
    candidate_id: &str,
) -> Result<DraftReconciliationCandidate, ServerError> {
    let mut tx = pool.begin().await?;
    let candidate =
        repository::load_reconciliation_candidate(&mut tx, draft_id, candidate_id).await?;
    tx.commit().await?;
    Ok(candidate)
}

pub async fn create_draft_rebase(
    pool: &sqlx::PgPool,
    draft_id: &str,
    author_user_id: &str,
    expected_ref: Option<&str>,
    request: CreateDraftRebaseRequest,
) -> Result<DraftRebaseResult, ServerError> {
    let mut tx = pool.begin().await?;
    let applied = repository::apply_draft_rebase_in_tx(
        &mut tx,
        draft_id,
        author_user_id,
        expected_ref,
        request,
    )
    .await?;
    tx.commit().await?;
    Ok(applied)
}

pub async fn update_draft(
    pool: &sqlx::PgPool,
    draft_id: &str,
    expected_draft_version: i64,
    request: UpdateDraftRequest,
) -> Result<DraftDetail, ServerError> {
    let mut tx = pool.begin().await?;
    repository::update_draft(&mut tx, draft_id, expected_draft_version, request).await?;
    tx.commit().await?;
    get_draft(pool, draft_id).await
}

pub async fn discard_draft(
    pool: &sqlx::PgPool,
    draft_id: &str,
    actor_user_id: &str,
    expected_draft_version: i64,
) -> Result<DeleteResult, ServerError> {
    let mut tx = pool.begin().await?;
    let result =
        repository::discard_draft(&mut tx, draft_id, actor_user_id, expected_draft_version).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn create_draft_operation_batch(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    request: DraftOperationBatchRequest,
) -> Result<DraftOperationBatchResponse, ServerError> {
    if request.operations.is_empty() {
        return Err(ServerError::InvalidRequest(
            "draft operation batch cannot be empty".to_owned(),
        ));
    }
    let draft_ids = request
        .operations
        .iter()
        .map(|item| item.draft_id.clone())
        .collect::<Vec<_>>();
    let mut tx = pool.begin().await?;
    repository::ensure_drafts_owned_by(&mut tx, principal, &draft_ids).await?;
    let result = repository::create_draft_operation_batch(&mut tx, request).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn list_draft_events(
    pool: &sqlx::PgPool,
    author_user_id: &str,
    after_cursor: Option<&str>,
    limit: Option<i64>,
) -> Result<DraftEventListResponse, ServerError> {
    let limit = limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ServerError::InvalidRequest(
            "draft event limit must be between 1 and 200".to_owned(),
        ));
    }
    after_cursor
        .map(str::parse::<i64>)
        .transpose()
        .map_err(|_| ServerError::InvalidRequest("invalid draft event cursor".to_owned()))?;
    repository::list_draft_events(pool, author_user_id, after_cursor, Some(limit)).await
}

pub(crate) async fn canonicalize_org_draft_targets_are_selected(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
    base_commit_id: Option<&str>,
    draft_resource: &mut DraftResourceRef,
    operations: &mut [DraftOperationInput],
) -> Result<(), ServerError> {
    if operations
        .first()
        .is_some_and(|operation| operation.action == DraftOperationAction::Create)
    {
        return Ok(());
    }
    if draft_resource.id.is_some() {
        canonicalize_org_draft_target_is_selected(
            tx,
            project_id,
            org_id,
            base_commit_id,
            draft_resource,
        )
        .await?;
    } else if draft_resource.path.is_some()
        && let Some(resource_id) = resolve_org_draft_target_id(
            tx,
            org_id,
            base_commit_id,
            draft_resource,
            !operations.is_empty(),
        )
        .await?
    {
        draft_resource.id = Some(resource_id);
        validate_org_draft_target_is_selected(tx, project_id, org_id, draft_resource).await?;
    }
    for operation in operations {
        if operation.action != DraftOperationAction::Create {
            canonicalize_org_draft_target_is_selected(
                tx,
                project_id,
                org_id,
                base_commit_id,
                &mut operation.resource,
            )
            .await?;
        }
    }
    Ok(())
}

pub(crate) async fn validate_org_draft_operation_inputs_are_selected(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
    base_commit_id: Option<&str>,
    operations: &[DraftOperationInput],
) -> Result<(), ServerError> {
    if operations
        .first()
        .is_some_and(|operation| operation.action == DraftOperationAction::Create)
    {
        return Ok(());
    }
    for operation in operations {
        if operation.action != DraftOperationAction::Create {
            validate_org_draft_target_is_selected_at_base(
                tx,
                project_id,
                org_id,
                base_commit_id,
                &operation.resource,
            )
            .await?;
        }
    }
    Ok(())
}

pub(crate) async fn validate_stored_org_draft_operations_are_selected(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
    base_commit_id: Option<&str>,
    operations: &[DraftOperation],
) -> Result<(), ServerError> {
    if operations
        .first()
        .is_some_and(|operation| operation.input.action == DraftOperationAction::Create)
    {
        return Ok(());
    }
    for operation in operations {
        if operation.input.action != DraftOperationAction::Create {
            validate_org_draft_target_is_selected_at_base(
                tx,
                project_id,
                org_id,
                base_commit_id,
                &operation.input.resource,
            )
            .await?;
        }
    }
    Ok(())
}

pub(crate) async fn canonicalize_org_draft_target_is_selected(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
    base_commit_id: Option<&str>,
    resource: &mut DraftResourceRef,
) -> Result<(), ServerError> {
    if resource.id.is_none() {
        resource.id =
            resolve_org_draft_target_id(tx, org_id, base_commit_id, resource, true).await?;
    }
    validate_org_draft_target_is_selected(tx, project_id, org_id, resource).await
}

pub(crate) async fn validate_org_draft_target_is_selected_at_base(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
    base_commit_id: Option<&str>,
    resource: &DraftResourceRef,
) -> Result<(), ServerError> {
    let mut canonical = resource.clone();
    canonicalize_org_draft_target_is_selected(
        tx,
        project_id,
        org_id,
        base_commit_id,
        &mut canonical,
    )
    .await
}

pub(crate) async fn draft_result_hash(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<String, ServerError> {
    state_hash(&draft_result_state(tx, draft_id).await?)
}

pub(crate) async fn target_ref_for_draft(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    scope: ResourceScope,
) -> Result<Option<String>, ServerError> {
    match scope {
        ResourceScope::Org => {
            let org_id = project_org_id(tx, project_id).await?;
            current_org_ref(tx, &org_id).await
        }
        ResourceScope::Project => current_project_ref(tx, project_id).await,
    }
}

pub(crate) async fn apply_operation(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    scope: ResourceScope,
    operation: &DraftOperationInput,
) -> Result<Option<String>, ServerError> {
    if operation.resource.scope != scope {
        return Err(ServerError::InvalidRequest(
            "draft operation scope does not match its draft".to_owned(),
        ));
    }
    apply_resource_operation(tx, project_id, scope, operation).await
}

// Transaction participants share the caller-owned transaction; only the outer service commits.
pub(crate) use super::repository::apply_draft_rebase_in_tx;
pub(crate) use super::repository::create_draft as create_draft_in_tx;
pub(crate) use super::repository::create_reconciliation_candidate_in_tx;
pub(crate) use super::repository::draft_result_state;
pub(crate) use super::repository::ensure_drafts_authored_by;
pub(crate) use super::repository::insert_draft_event;
pub(crate) use super::repository::invalidate_draft_candidates;
pub(crate) use super::repository::load_draft_detail;
pub(crate) use super::repository::load_draft_operations;
pub(crate) use super::repository::user_ref;
pub(crate) use super::repository::user_ref_from_row;
