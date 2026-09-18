//! HTTP extraction and response construction for draft resources.

use super::dto::{ListDraftEventsQuery, ListDraftsQuery};
use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::draft::dto::{
    CreateDraftRebaseRequest, CreateDraftReconciliationCandidateRequest, CreateDraftRequest,
    DraftOperationBatchRequest, DraftOperationInput, UpdateDraftRequest,
};
use crate::app::project;
use crate::http::{HttpError, parse_if_match, parse_ref_if_match};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;

pub(super) async fn create_draft(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<CreateDraftRequest>,
) -> Result<Json<dto::DraftDetail>, HttpError> {
    project::service::ensure_project_member(&state.pool, &principal, &request.project_id).await?;
    Ok(Json(
        service::create_draft(&state.pool, &principal.user_id, request).await?,
    ))
}

pub(super) async fn list_drafts(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<ListDraftsQuery>,
) -> Result<Json<dto::DraftListResponse>, HttpError> {
    Ok(Json(
        service::list_drafts(&state.pool, &principal.user_id, query.project_id.as_deref()).await?,
    ))
}

pub(super) async fn get_draft(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
) -> Result<Json<dto::DraftDetail>, HttpError> {
    service::ensure_draft_owner(&state.pool, &principal, &draft_id).await?;
    Ok(Json(service::get_draft(&state.pool, &draft_id).await?))
}

pub(super) async fn update_draft(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateDraftRequest>,
) -> Result<Json<dto::DraftDetail>, HttpError> {
    service::ensure_draft_owner(&state.pool, &principal, &draft_id).await?;
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        service::update_draft(&state.pool, &draft_id, expected_version, request).await?,
    ))
}

pub(super) async fn delete_draft(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    service::ensure_draft_owner(&state.pool, &principal, &draft_id).await?;
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        service::discard_draft(&state.pool, &draft_id, &principal.user_id, expected_version)
            .await?,
    ))
}

pub(super) async fn append_draft_operation(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DraftOperationInput>,
) -> Result<Json<dto::DraftDetail>, HttpError> {
    service::ensure_draft_owner(&state.pool, &principal, &draft_id).await?;
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        service::append_draft_operation(&state.pool, &draft_id, expected_version, request).await?,
    ))
}

pub(super) async fn create_draft_reconciliation_candidate(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    Json(request): Json<CreateDraftReconciliationCandidateRequest>,
) -> Result<Json<dto::DraftReconciliationCandidate>, HttpError> {
    service::ensure_draft_owner(&state.pool, &principal, &draft_id).await?;
    Ok(Json(
        service::create_draft_reconciliation_candidate(&state.pool, &draft_id, request).await?,
    ))
}

pub(super) async fn get_draft_reconciliation_candidate(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((draft_id, candidate_id)): Path<(String, String)>,
) -> Result<Json<dto::DraftReconciliationCandidate>, HttpError> {
    service::ensure_draft_owner(&state.pool, &principal, &draft_id).await?;
    Ok(Json(
        service::get_draft_reconciliation_candidate(&state.pool, &draft_id, &candidate_id).await?,
    ))
}

pub(super) async fn create_draft_rebase(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateDraftRebaseRequest>,
) -> Result<Json<dto::DraftRebaseResult>, HttpError> {
    service::ensure_draft_owner(&state.pool, &principal, &draft_id).await?;
    let expected_ref = parse_ref_if_match(&headers)?;
    Ok(Json(
        service::create_draft_rebase(
            &state.pool,
            &draft_id,
            &principal.user_id,
            expected_ref.as_deref(),
            request,
        )
        .await?,
    ))
}

pub(super) async fn list_draft_events(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<ListDraftEventsQuery>,
) -> Result<Json<dto::DraftEventListResponse>, HttpError> {
    Ok(Json(
        service::list_draft_events(
            &state.pool,
            &principal.user_id,
            query.after_cursor.as_deref(),
            query.limit,
        )
        .await?,
    ))
}

pub(super) async fn create_draft_operation_batch(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<DraftOperationBatchRequest>,
) -> Result<Json<dto::DraftOperationBatchResponse>, HttpError> {
    Ok(Json(
        service::create_draft_operation_batch(&state.pool, &principal, request).await?,
    ))
}
