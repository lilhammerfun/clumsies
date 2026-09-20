//! HTTP extraction and response construction for draft resources.

use super::dto::{ListDraftEventsQuery, ListDraftsQuery};
use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::draft::dto::{
    CreateDraftRebaseRequest, CreateDraftReconciliationCandidateRequest, CreateDraftRequest,
    DraftOperationBatchRequest, DraftOperationInput, UpdateDraftRequest,
};
use crate::http::{HttpError, parse_if_match, parse_ref_if_match};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;

/// Create an owned proposal after validating its resource, ancestor, and project membership.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_draft(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<CreateDraftRequest>,
) -> Result<Json<dto::DraftDetail>, HttpError> {
    Ok(Json(
        service::create_draft(&state.pool, &principal, request).await?,
    ))
}

/// Return proposals belonging to the authenticated author, optionally within one project.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_drafts(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<ListDraftsQuery>,
) -> Result<Json<dto::DraftListResponse>, HttpError> {
    Ok(Json(
        service::list_drafts(&state.pool, &principal, query.project_id.as_deref()).await?,
    ))
}

/// Return proposal details only after checking the caller's ownership.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_draft(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
) -> Result<Json<dto::DraftDetail>, HttpError> {
    Ok(Json(
        service::get_draft(&state.pool, &principal, &draft_id).await?,
    ))
}

/// Change owned proposal metadata only at the expected mutable lifecycle revision.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn update_draft(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateDraftRequest>,
) -> Result<Json<dto::DraftDetail>, HttpError> {
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        service::update_draft(
            &state.pool,
            &principal,
            &draft_id,
            expected_version,
            request,
        )
        .await?,
    ))
}

/// Adapt a version-preconditioned HTTP deletion into proposal discard.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn delete_draft(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        service::discard_draft(&state.pool, &draft_id, &principal, expected_version).await?,
    ))
}

/// Append a validated mutation at the expected proposal revision and persist its event.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn append_draft_operation(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DraftOperationInput>,
) -> Result<Json<dto::DraftDetail>, HttpError> {
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        service::append_draft_operation(
            &state.pool,
            &principal,
            &draft_id,
            expected_version,
            request,
        )
        .await?,
    ))
}

/// Compute and persist three-way reconciliation for an owned proposal revision.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_draft_reconciliation_candidate(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    Json(request): Json<CreateDraftReconciliationCandidateRequest>,
) -> Result<Json<dto::DraftReconciliationCandidate>, HttpError> {
    Ok(Json(
        service::create_draft_reconciliation_candidate(&state.pool, &principal, &draft_id, request)
            .await?,
    ))
}

/// Return an owned proposal's candidate with validity checked against current references.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_draft_reconciliation_candidate(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((draft_id, candidate_id)): Path<(String, String)>,
) -> Result<Json<dto::DraftReconciliationCandidate>, HttpError> {
    Ok(Json(
        service::get_draft_reconciliation_candidate(
            &state.pool,
            &principal,
            &draft_id,
            &candidate_id,
        )
        .await?,
    ))
}

/// Apply an owned candidate or explicit conflict resolution and preserve the previous revision.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_draft_rebase(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateDraftRebaseRequest>,
) -> Result<Json<dto::DraftRebaseResult>, HttpError> {
    let expected_ref = parse_ref_if_match(&headers)?;
    Ok(Json(
        service::create_draft_rebase(
            &state.pool,
            &draft_id,
            &principal,
            expected_ref.as_deref(),
            request,
        )
        .await?,
    ))
}

/// Resume the authenticated author's lifecycle feed after validating cursor and page bounds.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_draft_events(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<ListDraftEventsQuery>,
) -> Result<Json<dto::DraftEventListResponse>, HttpError> {
    Ok(Json(
        service::list_draft_events(
            &state.pool,
            &principal,
            query.after_cursor.as_deref(),
            query.limit,
        )
        .await?,
    ))
}

/// Apply an authenticated author's ordered mutations atomically across all affected drafts.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_draft_operation_batch(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<DraftOperationBatchRequest>,
) -> Result<Json<dto::DraftOperationBatchResponse>, HttpError> {
    Ok(Json(
        service::create_draft_operation_batch(&state.pool, &principal, request).await?,
    ))
}
