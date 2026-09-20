//! Draft use cases, reconciliation, and transaction coordination.

use super::model::{
    apply_operations_to_state, diff_resource_states, draft_status, ensure_writable_draft_scope,
    merge_resource_states, state_hash, validate_draft_operation_resource, validate_draft_resource,
    validate_new_resource_draft_operations,
};
use super::repository;
use super::repository::{
    insert_draft_operation, path_is_occupied, resolve_org_draft_target_id,
    resource_state_at_commit, validate_org_draft_target_is_selected,
};
use crate::app::auth::AuthPrincipal;
use crate::app::commit::{
    current_org_ref, current_project_ref, load_org_ref, load_project_ref, validate_org_commit,
    validate_project_commit,
};
use crate::app::draft::dto::{
    CreateDraftRebaseRequest, CreateDraftReconciliationCandidateRequest, CreateDraftRequest, Draft,
    DraftCoordination, DraftDetail, DraftEventListResponse, DraftEventType, DraftFreshness,
    DraftListResponse, DraftOperation, DraftOperationAction, DraftOperationBatchRequest,
    DraftOperationBatchResponse, DraftOperationInput, DraftRebaseResult,
    DraftReconciliationCandidate, DraftReconciliationStatus, DraftResourceRef, DraftSyncState,
    DraftSyncStatus, ReconciliationCandidateStatus, ReconciliationConflict,
    ReconciliationConflictKind, ReconciliationResourceState, UpdateDraftRequest,
};
use crate::app::memory::dto::ResourceScope;
use crate::app::memory::model::resource_scope;
use crate::app::memory::{
    apply_resource_operation, lock_org_draft_selection_coordination,
    lock_org_draft_selection_coordination_for_project, project_org_id,
};
use crate::app::review::{
    load_review, load_review_draft_ids, refresh_review_after_draft_content_change,
    review_result_hash,
};
use crate::dto::DeleteResult;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;

/// Hide proposals that do not belong to the principal within its accessible projects.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
async fn ensure_draft_owner(
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

/// Create an owned proposal after validating its resource, ancestor, and project membership.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn create_draft(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    request: CreateDraftRequest,
) -> Result<DraftDetail, ServerError> {
    let author_user_id = &principal.user_id;
    crate::app::project::ensure_project_member(pool, principal, &request.project_id).await?;

    let mut tx = pool.begin().await?;
    let draft_id = create_draft_in_tx(&mut tx, author_user_id, request).await?;
    tx.commit().await?;
    get_draft(pool, principal, &draft_id).await
}

/// Return proposals belonging to the authenticated author, optionally within one project.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_drafts(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: Option<&str>,
) -> Result<DraftListResponse, ServerError> {
    let author_user_id = &principal.user_id;

    repository::list_drafts(pool, author_user_id, project_id).await
}

/// Return proposal details only after checking the caller's ownership.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_draft(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    draft_id: &str,
) -> Result<DraftDetail, ServerError> {
    ensure_draft_owner(pool, principal, draft_id).await?;

    let mut tx = pool.begin().await?;
    let detail = load_draft_detail(&mut tx, draft_id).await?;
    tx.commit().await?;
    Ok(detail)
}

/// Append a validated mutation at the expected proposal revision and persist its event.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, stale revisions or reference
/// preconditions, and propagates persistence failures.
pub async fn append_draft_operation(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    draft_id: &str,
    expected_draft_version: i64,
    operation: DraftOperationInput,
) -> Result<DraftDetail, ServerError> {
    ensure_draft_owner(pool, principal, draft_id).await?;

    let mut tx = pool.begin().await?;
    append_draft_operation_in_tx(
        &mut tx,
        draft_id,
        expected_draft_version,
        operation,
        None,
        false,
    )
    .await?;
    tx.commit().await?;
    get_draft(pool, principal, draft_id).await
}

/// Compute and persist three-way reconciliation for an owned proposal revision.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, stale revisions or reference
/// preconditions, and propagates persistence failures.
pub async fn create_draft_reconciliation_candidate(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    draft_id: &str,
    request: CreateDraftReconciliationCandidateRequest,
) -> Result<DraftReconciliationCandidate, ServerError> {
    ensure_draft_owner(pool, principal, draft_id).await?;

    let mut tx = pool.begin().await?;
    let candidate =
        create_reconciliation_candidate_in_tx(&mut tx, draft_id, request.expected_draft_version)
            .await?;
    tx.commit().await?;
    Ok(candidate)
}

/// Return an owned proposal's candidate with validity checked against current references.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_draft_reconciliation_candidate(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    draft_id: &str,
    candidate_id: &str,
) -> Result<DraftReconciliationCandidate, ServerError> {
    ensure_draft_owner(pool, principal, draft_id).await?;

    let mut tx = pool.begin().await?;
    let candidate = load_reconciliation_candidate(&mut tx, draft_id, candidate_id).await?;
    tx.commit().await?;
    Ok(candidate)
}

/// Apply an owned candidate or explicit conflict resolution and preserve the previous revision.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, stale revisions or reference
/// preconditions, and propagates persistence failures.
pub async fn create_draft_rebase(
    pool: &sqlx::PgPool,
    draft_id: &str,
    principal: &AuthPrincipal,
    expected_ref: Option<&str>,
    request: CreateDraftRebaseRequest,
) -> Result<DraftRebaseResult, ServerError> {
    let author_user_id = &principal.user_id;
    ensure_draft_owner(pool, principal, draft_id).await?;

    let mut tx = pool.begin().await?;
    let applied =
        apply_draft_rebase_in_tx(&mut tx, draft_id, author_user_id, expected_ref, request).await?;
    tx.commit().await?;
    Ok(applied)
}

/// Change owned proposal metadata only at the expected mutable lifecycle revision.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, stale revisions or reference
/// preconditions, and propagates persistence failures.
pub async fn update_draft(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    draft_id: &str,
    expected_draft_version: i64,
    request: UpdateDraftRequest,
) -> Result<DraftDetail, ServerError> {
    ensure_draft_owner(pool, principal, draft_id).await?;

    let mut tx = pool.begin().await?;
    update_draft_in_tx(&mut tx, draft_id, expected_draft_version, request).await?;
    tx.commit().await?;
    get_draft(pool, principal, draft_id).await
}

/// Discard an owned editable proposal, invalidate candidates, and remove it from its pending review.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, stale revisions or reference
/// preconditions, and propagates persistence failures.
pub async fn discard_draft(
    pool: &sqlx::PgPool,
    draft_id: &str,
    principal: &AuthPrincipal,
    expected_draft_version: i64,
) -> Result<DeleteResult, ServerError> {
    let actor_user_id = &principal.user_id;
    ensure_draft_owner(pool, principal, draft_id).await?;

    let mut tx = pool.begin().await?;
    let result =
        discard_draft_in_tx(&mut tx, draft_id, actor_user_id, expected_draft_version).await?;
    tx.commit().await?;
    Ok(result)
}

/// Apply an authenticated author's ordered mutations atomically across all affected drafts.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
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
    let result = create_draft_operation_batch_in_tx(&mut tx, request).await?;
    tx.commit().await?;
    Ok(result)
}

/// Resume the authenticated author's lifecycle feed after validating cursor and page bounds.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
pub async fn list_draft_events(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    after_cursor: Option<&str>,
    limit: Option<i64>,
) -> Result<DraftEventListResponse, ServerError> {
    let author_user_id = &principal.user_id;

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

/// Resolve stable target identities and require the project to select every edited organization
/// resource.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
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

/// Check proposed organization mutations against the carrying project's selected resources.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
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

/// Require persisted organization mutations to remain within the carrying project's selected
/// resources.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
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

/// Resolve an organization target's stable identity before checking its project selection.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
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

/// Require the selected target identity to exist at the proposal's ancestor snapshot.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
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

/// Fingerprint the proposal's final resource state for reconciliation and approval checks.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub(crate) async fn draft_result_hash(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<String, ServerError> {
    state_hash(&draft_result_state(tx, draft_id).await?)
}

/// Read the authoritative reference belonging to a proposal's ownership scope.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
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

/// Require a proposal mutation to match its scope before applying the resource change.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
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
use super::repository::insert_draft_event;
use super::repository::invalidate_draft_candidates;
use super::repository::load_draft_operations;
use super::repository::user_ref;

/// Coordinate selection locks, revision checks, mutation persistence, and review approval
/// refresh.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, invalid input or lifecycle state, and
/// propagates persistence failures.
pub(crate) async fn append_draft_operation_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    expected_draft_version: i64,
    mut operation: DraftOperationInput,
    event_daemon_installation_id: Option<&str>,
    org_coordination_already_locked: bool,
) -> Result<i64, ServerError> {
    let identity = repository::load_identity(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let identity_scope = resource_scope(identity.resource_scope.clone().as_str())?;
    ensure_writable_draft_scope(identity_scope)?;
    if identity_scope == ResourceScope::Org && !org_coordination_already_locked {
        lock_org_draft_selection_coordination_for_project(tx, &identity.project_id.clone()).await?;
    }
    let row = repository::lock_append_state(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let status: String = row.status.clone();
    let version: i64 = row.version;
    let scope = resource_scope(row.resource_scope.clone().as_str())?;
    let draft_resource = DraftResourceRef {
        scope,
        id: None,
        path: None,
    };

    if status != "open" && status != "submitted" {
        return Err(ServerError::invalid_transition("draft", &status, "append"));
    }
    if version != expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            expected_draft_version,
            version,
        ));
    }
    let creates_resource: bool = row.creates_resource;
    if creates_resource && operation.action == DraftOperationAction::Delete {
        return Err(ServerError::InvalidRequest(
            "a draft-created resource must be discarded instead of deleted".to_owned(),
        ));
    }
    validate_draft_operation_resource(&draft_resource, &operation)?;
    if scope == ResourceScope::Org
        && !creates_resource
        && operation.action != DraftOperationAction::Create
    {
        let project_id: String = row.project_id.clone();
        let org_id = project_org_id(tx, &project_id).await?;
        let base_commit_id: Option<String> = row.base_commit_id.clone();
        canonicalize_org_draft_target_is_selected(
            tx,
            &project_id,
            &org_id,
            base_commit_id.as_deref(),
            &mut operation.resource,
        )
        .await?;
    }

    insert_draft_operation(tx, draft_id, operation).await?;
    let updated = repository::increment_version(tx, draft_id).await?;
    invalidate_draft_candidates(tx, draft_id).await?;
    refresh_review_after_draft_content_change(tx, draft_id).await?;
    insert_draft_event(
        tx,
        draft_id,
        &updated.project_id.clone(),
        DraftEventType::OperationAppended,
        updated.version,
        event_daemon_installation_id,
    )
    .await
}

/// Assemble a proposal's ordered mutations and current reconciliation state within the caller's
/// transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub(crate) async fn load_draft_detail(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<DraftDetail, ServerError> {
    let row = repository::load_detail_record(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;

    let daemon_installation_id: String = row.daemon_installation_id.clone();
    let resource_scope = resource_scope(row.resource_scope.clone().as_str())?;
    let resource = DraftResourceRef {
        scope: resource_scope,
        id: row.target_id.clone(),
        path: row.path.clone(),
    };
    let operations = load_draft_operations(tx, draft_id).await?;
    let allow_path_lookup = operations
        .first()
        .is_none_or(|operation| operation.input.action != DraftOperationAction::Create);
    let coordination = load_draft_coordination(
        tx,
        draft_id,
        row.project_id.clone(),
        row.base_commit_id.clone(),
        row.version,
        &resource,
        allow_path_lookup,
    )
    .await?;
    let draft = Draft {
        draft_id: row.draft_id.clone(),
        project_id: row.project_id.clone(),
        base_commit_id: row.base_commit_id.clone(),
        author: row.author.clone(),
        title: row.title.clone(),
        description: row.description.clone(),
        resource,
        status: draft_status(row.status.clone().as_str())?,
        coordination,
        version: row.version,
        created_at: row.created_at,
        updated_at: row.updated_at,
    };
    Ok(DraftDetail {
        draft,
        operations,
        sync_state: DraftSyncState {
            status: DraftSyncStatus::Synced,
            server_cursor: Some(format!("draft:{}:{}", draft_id, row.version.clone())),
            daemon_installation_id: Some(daemon_installation_id),
        },
    })
}

/// Compare the proposal's ancestor with its current reference and locate matching reconciliation
/// evidence.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
pub(crate) async fn load_draft_coordination(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    project_id: String,
    base_commit_id: Option<String>,
    draft_version: i64,
    resource: &DraftResourceRef,
    allow_path_lookup: bool,
) -> Result<DraftCoordination, ServerError> {
    let current_commit_id = match resource.scope {
        ResourceScope::Org => {
            let org_id = project_org_id(tx, &project_id).await?;
            load_org_ref(tx, &org_id).await?.commit_id
        }
        ResourceScope::Project => load_project_ref(tx, &project_id).await?.commit_id,
    };
    let freshness = if base_commit_id == current_commit_id {
        DraftFreshness::Current
    } else {
        DraftFreshness::Behind
    };
    let has_upstream_resource_changes = if freshness == DraftFreshness::Behind {
        let base_state =
            resource_state_at_commit(tx, base_commit_id.as_deref(), resource, allow_path_lookup)
                .await?;
        let current_state = resource_state_at_commit(
            tx,
            current_commit_id.as_deref(),
            resource,
            allow_path_lookup,
        )
        .await?;
        base_state != current_state
    } else {
        false
    };
    let candidate = if freshness == DraftFreshness::Behind {
        repository::find_candidate_summary(
            tx,
            draft_id,
            draft_version,
            &base_commit_id,
            &current_commit_id,
        )
        .await?
    } else {
        None
    };
    let (reconciliation, candidate_id) = match candidate {
        Some(row) => {
            let status: String = row.status.clone();
            (
                match status.as_str() {
                    "clean" => DraftReconciliationStatus::Clean,
                    "conflicts" => DraftReconciliationStatus::Conflicts,
                    _ => {
                        return Err(ServerError::InvalidRequest(format!(
                            "unknown reconciliation status: {status}"
                        )));
                    }
                },
                Some(row.candidate_id.clone()),
            )
        }
        None => (DraftReconciliationStatus::Unknown, None),
    };
    Ok(DraftCoordination {
        freshness,
        current_commit_id,
        has_upstream_resource_changes,
        reconciliation,
        candidate_id,
    })
}

/// Apply the proposal's ordered mutations to its ancestor to compute final resource state.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub(crate) async fn draft_result_state(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<ReconciliationResourceState, ServerError> {
    let row = repository::load_base_state(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let operations = load_draft_operations(tx, draft_id).await?;
    let resource = DraftResourceRef {
        scope: resource_scope(row.resource_scope.clone().as_str())?,
        id: row.target_id.clone(),
        path: row.path.clone(),
    };
    let allow_path_lookup = operations
        .first()
        .is_none_or(|operation| operation.input.action != DraftOperationAction::Create);
    let base_commit_id: Option<String> = row.base_commit_id.clone();
    let base =
        resource_state_at_commit(tx, base_commit_id.as_deref(), &resource, allow_path_lookup)
            .await?;
    apply_operations_to_state(base, &operations)
}

/// Persist a reusable three-way result for the exact proposal and upstream revisions.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, invalid input or lifecycle state, and
/// propagates persistence failures.
pub(crate) async fn create_reconciliation_candidate_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    expected_draft_version: i64,
) -> Result<DraftReconciliationCandidate, ServerError> {
    let row = repository::lock_reconciliation_state(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let draft_version: i64 = row.version;
    if draft_version != expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            expected_draft_version,
            draft_version,
        ));
    }
    let lifecycle: String = row.status.clone();
    if lifecycle != "open" && lifecycle != "submitted" {
        return Err(ServerError::invalid_transition(
            "draft",
            &lifecycle,
            "reconciled",
        ));
    }
    let project_id: String = row.project_id.clone();
    let scope = resource_scope(row.resource_scope.clone().as_str())?;
    let base_commit_id: Option<String> = row.base_commit_id.clone();
    let current_commit_id = target_ref_for_draft(tx, &project_id, scope).await?;
    if base_commit_id == current_commit_id {
        return Err(ServerError::DraftAlreadyCurrent {
            draft_id: draft_id.to_owned(),
        });
    }

    if let Some(existing_id) = repository::find_current_candidate(
        tx,
        draft_id,
        draft_version,
        &base_commit_id,
        &current_commit_id,
    )
    .await?
    {
        return load_reconciliation_candidate(tx, draft_id, &existing_id).await;
    }

    invalidate_draft_candidates(tx, draft_id).await?;
    let mut resource = DraftResourceRef {
        scope,
        id: row.target_id.clone(),
        path: row.path.clone(),
    };
    let operations = load_draft_operations(tx, draft_id).await?;
    let allow_path_lookup = operations
        .first()
        .is_none_or(|operation| operation.input.action != DraftOperationAction::Create);
    if scope == ResourceScope::Org && resource.id.is_none() && allow_path_lookup {
        let org_id = project_org_id(tx, &project_id).await?;
        resource.id =
            resolve_org_draft_target_id(tx, &org_id, base_commit_id.as_deref(), &resource, true)
                .await?;
    }
    let base_state =
        resource_state_at_commit(tx, base_commit_id.as_deref(), &resource, allow_path_lookup)
            .await?;
    let current_state = resource_state_at_commit(
        tx,
        current_commit_id.as_deref(),
        &resource,
        allow_path_lookup,
    )
    .await?;
    let draft_state = apply_operations_to_state(base_state.clone(), &operations)?;
    let (mut proposed_state, mut conflicts) =
        merge_resource_states(&base_state, &current_state, &draft_state);
    if let Some(proposed) = proposed_state.as_ref()
        && path_is_occupied(tx, current_commit_id.as_deref(), proposed).await?
    {
        conflicts.push(ReconciliationConflict {
            kind: ReconciliationConflictKind::PathOccupied,
            field: "path".to_owned(),
            base: base_state.resource.path.clone(),
            current: current_state.resource.path.clone(),
            draft: proposed.resource.path.clone(),
        });
        proposed_state = None;
    }
    let status = if conflicts.is_empty() {
        ReconciliationCandidateStatus::Clean
    } else {
        ReconciliationCandidateStatus::Conflicts
    };
    let result_hash = proposed_state.as_ref().map(state_hash).transpose()?;
    let candidate_id = prefixed_id("rcn");
    repository::insert_candidate(
        tx,
        repository::NewCandidate {
            candidate_id: &candidate_id,
            draft_id,
            draft_version,
            base_commit_id: &base_commit_id,
            current_commit_id: &current_commit_id,
            status: match status {
                ReconciliationCandidateStatus::Clean => "clean",
                ReconciliationCandidateStatus::Conflicts => "conflicts",
            },
            base_state: &base_state,
            current_state: &current_state,
            draft_state: &draft_state,
            proposed_state: proposed_state.as_ref(),
            conflicts: &conflicts,
            result_hash: &result_hash,
        },
    )
    .await?;
    load_reconciliation_candidate(tx, draft_id, &candidate_id).await
}

/// Save the prior proposal, apply resolved upstream state, and invalidate approval only when
/// content changes.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, invalid input or lifecycle state, and
/// propagates persistence failures.
pub(crate) async fn apply_draft_rebase_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    author_user_id: &str,
    expected_ref: Option<&str>,
    request: CreateDraftRebaseRequest,
) -> Result<DraftRebaseResult, ServerError> {
    let identity = repository::load_identity(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    lock_org_draft_selection_coordination_for_project(tx, &identity.project_id).await?;
    let row = repository::lock_rebase_state(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    if row.author_user_id.clone() != author_user_id {
        return Err(ServerError::Forbidden(
            "only the draft author can rebase it".to_owned(),
        ));
    }
    let version: i64 = row.version;
    if version != request.expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            request.expected_draft_version,
            version,
        ));
    }
    let lifecycle: String = row.status.clone();
    if lifecycle != "open" && lifecycle != "submitted" {
        return Err(ServerError::invalid_transition(
            "draft", &lifecycle, "rebased",
        ));
    }
    let candidate = load_reconciliation_candidate(tx, draft_id, &request.candidate_id).await?;
    if !candidate.valid
        || candidate.draft_version != version
        || candidate.base_commit_id != row.base_commit_id.clone()
    {
        return Err(ServerError::ReconciliationCandidateInvalid {
            candidate_id: request.candidate_id,
        });
    }
    let project_id: String = row.project_id.clone();
    let scope = resource_scope(row.resource_scope.clone().as_str())?;
    let current_ref = target_ref_for_draft(tx, &project_id, scope).await?;
    if current_ref.as_deref() != expected_ref {
        return Err(ServerError::precondition_failed(
            expected_ref,
            current_ref.as_deref(),
        ));
    }
    if candidate.current_commit_id != current_ref {
        return Err(ServerError::ReconciliationCandidateInvalid {
            candidate_id: request.candidate_id,
        });
    }
    let resolved_state = match (candidate.status, request.resolved_state) {
        (ReconciliationCandidateStatus::Clean, Some(_)) => {
            return Err(ServerError::InvalidRequest(
                "a clean candidate must be applied without resolved_state".to_owned(),
            ));
        }
        (ReconciliationCandidateStatus::Clean, None) => {
            candidate.proposed_state.clone().ok_or_else(|| {
                ServerError::InvalidRequest("clean candidate has no result".to_owned())
            })?
        }
        (ReconciliationCandidateStatus::Conflicts, Some(resolved)) => resolved,
        (ReconciliationCandidateStatus::Conflicts, None) => {
            return Err(ServerError::InvalidRequest(
                "a conflicts candidate requires a resolved_state".to_owned(),
            ));
        }
    };
    if resolved_state.resource.scope != scope
        || (resolved_state.exists && resolved_state.content.is_none())
    {
        return Err(ServerError::InvalidRequest(
            "resolved state does not match the draft resource".to_owned(),
        ));
    }
    if path_is_occupied(tx, current_ref.as_deref(), &resolved_state).await? {
        return Err(ServerError::InvalidRequest(
            "resolved state path is occupied in the current commit".to_owned(),
        ));
    }

    let previous_operations = load_draft_operations(tx, draft_id).await?;
    let previous_revision_id = prefixed_id("drv");
    repository::insert_revision(
        tx,
        repository::NewDraftRevision {
            revision_id: &previous_revision_id,
            draft_id,
            draft_version: version,
            base_commit_id: row.base_commit_id.clone(),
            lifecycle_status: &lifecycle,
            title: row.title.clone(),
            description: row.description.clone(),
            operations: &previous_operations,
        },
    )
    .await?;

    let operations = diff_resource_states(&candidate.current_state, &resolved_state);
    repository::delete_operations(tx, draft_id).await?;
    for operation in operations {
        insert_draft_operation(tx, draft_id, operation).await?;
    }
    let next_version: i64 = repository::advance_base(tx, draft_id, &current_ref).await?;
    invalidate_draft_candidates(tx, draft_id).await?;
    let result_hash = state_hash(&resolved_state)?;
    let review_row = repository::lock_review_approval(tx, draft_id).await?;
    let mut approval_invalidated = false;
    if let Some(review_row) = review_row {
        let status: String = review_row.status.clone();
        let approved_hash: Option<String> = review_row.approved_result_hash.clone();
        let review_id: String = review_row.review_id.clone();
        let draft_ids = load_review_draft_ids(tx, &review_id).await?;
        let current_review_hash = review_result_hash(tx, &draft_ids).await?;
        let preserve_approval =
            status == "approved" && approved_hash.as_deref() == Some(&current_review_hash);
        approval_invalidated = status == "approved" && !preserve_approval;
        repository::update_review_approval(tx, &review_id, preserve_approval).await?;
    }
    let rebase_id = prefixed_id("rbs");
    repository::insert_rebase(
        tx,
        repository::NewDraftRebase {
            rebase_id: &rebase_id,
            draft_id,
            candidate_id: &candidate.candidate_id,
            previous_revision_id: &previous_revision_id,
            author_user_id,
            next_version,
            result_hash: &result_hash,
        },
    )
    .await?;
    insert_draft_event(
        tx,
        draft_id,
        &project_id,
        DraftEventType::Rebased,
        next_version,
        None,
    )
    .await?;
    let draft = load_draft_detail(tx, draft_id).await?;
    let review = match repository::find_review(tx, draft_id).await? {
        Some(review_id) => Some(load_review(tx, &review_id).await?),
        None => None,
    };
    Ok(DraftRebaseResult {
        rebase_id,
        previous_revision_id,
        draft,
        review,
        approval_invalidated,
    })
}

/// Validate proposal invariants and persist its initial operations under selection coordination
/// locks.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
pub(crate) async fn create_draft_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    author_user_id: &str,
    mut request: CreateDraftRequest,
) -> Result<String, ServerError> {
    ensure_writable_draft_scope(request.resource.scope)?;
    if request.resource.scope == ResourceScope::Org {
        lock_org_draft_selection_coordination_for_project(tx, &request.project_id).await?;
    }
    let org_id = project_org_id(tx, &request.project_id).await?;
    user_ref(tx, author_user_id).await?;
    if let Some(base_commit_id) = request.base_commit_id.as_deref() {
        match request.resource.scope {
            ResourceScope::Org => validate_org_commit(tx, &org_id, base_commit_id).await?,
            ResourceScope::Project => {
                validate_project_commit(tx, &request.project_id, base_commit_id).await?
            }
        }
    }
    validate_draft_resource(&request.resource)?;
    for operation in &request.operations {
        validate_draft_operation_resource(&request.resource, operation)?;
    }
    validate_new_resource_draft_operations(&request.operations)?;
    if request.resource.scope == ResourceScope::Org {
        canonicalize_org_draft_targets_are_selected(
            tx,
            &request.project_id,
            &org_id,
            request.base_commit_id.as_deref(),
            &mut request.resource,
            &mut request.operations,
        )
        .await?;
    }

    let draft_id = prefixed_id("drf");
    repository::insert_draft(
        tx,
        repository::NewDraft {
            draft_id: &draft_id,
            project_id: &request.project_id,
            author_user_id,
            title: &request.title,
            description: request.description.as_deref().unwrap_or_default(),
            scope: request.resource.scope.as_str(),
            resource_kind: "memory",
            base_commit_id: &request.base_commit_id,
            target_id: &request.resource.id,
            path: &request.resource.path,
            daemon_installation_id: &request.daemon_installation_id,
        },
    )
    .await?;

    for operation in request.operations {
        insert_draft_operation(tx, &draft_id, operation).await?;
    }
    insert_draft_event(
        tx,
        &draft_id,
        &request.project_id,
        DraftEventType::Created,
        1,
        Some(&request.daemon_installation_id),
    )
    .await?;

    Ok(draft_id)
}

/// Check proposal revision and lifecycle before persisting metadata and a synchronization event.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, invalid input or lifecycle state, and
/// propagates persistence failures.
pub(crate) async fn update_draft_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    expected_draft_version: i64,
    request: UpdateDraftRequest,
) -> Result<(), ServerError> {
    let row = repository::lock_metadata(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let current_version: i64 = row.version;
    if current_version != expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            expected_draft_version,
            current_version,
        ));
    }
    let status = row.status.clone();
    if status != "open" && status != "submitted" {
        return Err(ServerError::invalid_transition("draft", &status, "updated"));
    }
    let existing_title: String = row.title.clone();
    let existing_description: String = row.description.clone();
    let title = request.title.unwrap_or(existing_title);
    let description = request.description.unwrap_or(existing_description);
    let updated = repository::update_metadata(tx, draft_id, title, description).await?;
    invalidate_draft_candidates(tx, draft_id).await?;
    insert_draft_event(
        tx,
        draft_id,
        &updated.project_id.clone(),
        DraftEventType::Updated,
        updated.version,
        None,
    )
    .await?;
    Ok(())
}

/// Transition a mutable proposal to discarded and invalidate its review evidence atomically.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, invalid input or lifecycle state, and
/// propagates persistence failures.
pub(crate) async fn discard_draft_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    actor_user_id: &str,
    expected_draft_version: i64,
) -> Result<DeleteResult, ServerError> {
    let identity = repository::load_identity(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    lock_org_draft_selection_coordination_for_project(tx, &identity.project_id).await?;
    let row = repository::lock_discard_state(tx, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", draft_id))?;
    let version: i64 = row.version;
    if version != expected_draft_version {
        return Err(ServerError::version_conflict(
            "draft",
            expected_draft_version,
            version,
        ));
    }
    let status: String = row.status.clone();
    if status != "open" && status != "submitted" {
        return Err(ServerError::invalid_transition(
            "draft",
            &status,
            "discarded",
        ));
    }
    let next_version: i64 = repository::mark_discarded(tx, draft_id).await?;
    invalidate_draft_candidates(tx, draft_id).await?;
    crate::app::review::remove_discarded_draft(tx, draft_id, actor_user_id).await?;
    insert_draft_event(
        tx,
        draft_id,
        &row.project_id.clone(),
        DraftEventType::Discarded,
        next_version,
        None,
    )
    .await?;
    Ok(DeleteResult {
        deleted: true,
        id: draft_id.to_owned(),
    })
}

/// Acquire organization coordination locks in stable order before applying the entire mutation
/// batch.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, invalid input or lifecycle state, and
/// propagates persistence failures.
pub(crate) async fn create_draft_operation_batch_in_tx(
    tx: &mut Transaction<'_, Postgres>,
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
    let rows = repository::list_batch_organizations(tx, &draft_ids).await?;
    for row in rows {
        lock_org_draft_selection_coordination(tx, &row.org_id.clone()).await?;
    }
    let mut accepted_operations = Vec::new();
    let mut cursor = None;
    let daemon_installation_id = request.daemon_installation_id;
    for item in request.operations {
        cursor = Some(
            append_draft_operation_in_tx(
                tx,
                &item.draft_id,
                item.expected_draft_version,
                item.operation,
                Some(&daemon_installation_id),
                true,
            )
            .await?,
        );
        accepted_operations.push(item.local_operation_id);
    }
    Ok(DraftOperationBatchResponse {
        cursor: cursor.expect("non-empty batch").to_string(),
        accepted_operations,
    })
}

/// Recheck a persisted candidate against the current proposal and reference, invalidating stale
/// evidence.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
pub(crate) async fn load_reconciliation_candidate(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    candidate_id: &str,
) -> Result<DraftReconciliationCandidate, ServerError> {
    let row = repository::load_candidate_record(tx, candidate_id, draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("reconciliation_candidate", candidate_id))?;

    let draft_row = repository::load_candidate_draft_state(tx, draft_id).await?;
    let scope = resource_scope(draft_row.resource_scope.clone().as_str())?;
    let current = target_ref_for_draft(tx, &draft_row.project_id.clone(), scope).await?;
    let invalidated_at: Option<OffsetDateTime> = row.invalidated_at;
    let valid = invalidated_at.is_none()
        && row.draft_version == draft_row.version
        && row.base_commit_id.clone() == draft_row.base_commit_id.clone()
        && row.current_commit_id.clone() == current;
    if !valid && invalidated_at.is_none() {
        repository::invalidate_candidate(tx, candidate_id).await?;
    }
    let status: String = row.status.clone();
    Ok(DraftReconciliationCandidate {
        candidate_id: row.candidate_id.clone(),
        draft_id: row.draft_id.clone(),
        draft_version: row.draft_version,
        base_commit_id: row.base_commit_id.clone(),
        current_commit_id: row.current_commit_id.clone(),
        status: match status.as_str() {
            "clean" => ReconciliationCandidateStatus::Clean,
            "conflicts" => ReconciliationCandidateStatus::Conflicts,
            _ => {
                return Err(ServerError::InvalidRequest(format!(
                    "unknown candidate status: {status}"
                )));
            }
        },
        base_state: row.base_state.clone().0,
        current_state: row.current_state.clone().0,
        draft_state: row.draft_state.clone().0,
        proposed_state: row.proposed_state.clone().map(|state| state.0),
        conflicts: row.conflicts.clone().0,
        merge_preview: (row.status == "conflicts").then(|| {
            super::model::reconciliation_merge_preview(
                &row.base_state.0,
                &row.current_state.0,
                &row.draft_state.0,
            )
        }),
        result_hash: row.result_hash.clone(),
        valid,
        created_at: row.created_at,
        invalidated_at: if valid {
            None
        } else {
            invalidated_at.or_else(|| Some(OffsetDateTime::now_utc()))
        },
    })
}
