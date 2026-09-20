//! HTTP extraction and response construction for review resources.

use super::dto::ListReviewsQuery;
use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::review::dto::{
    CreateReviewCommentRequest, CreateReviewDecisionRequest, CreateReviewMergeRequest,
    CreateReviewRequest, CreateReviewSubmissionRequest,
};
use crate::http::{HttpError, parse_ref_if_match};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;

/// Prepare every pending proposal against the same shared reference for its author.
///
/// # Errors
/// Rejects inaccessible reviews, non-authors, stale revisions, or closed reviews.
pub(super) async fn create_review_update_plan(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    Json(request): Json<dto::CreateReviewUpdatePlanRequest>,
) -> Result<Json<dto::ReviewUpdatePlan>, HttpError> {
    Ok(Json(
        service::create_review_update_plan(&state.pool, &principal, &review_id, request).await?,
    ))
}

/// Apply the complete inspected review update atomically without publishing it.
///
/// # Errors
/// Rejects unauthorized, stale, incomplete, or invalid resolutions without partial writes.
pub(super) async fn create_review_update(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<dto::CreateReviewUpdateRequest>,
) -> Result<Json<dto::ReviewDetail>, HttpError> {
    let expected_ref = parse_ref_if_match(&headers)?;
    Ok(Json(
        service::create_review_update(
            &state.pool,
            &principal,
            &review_id,
            expected_ref.as_deref(),
            request,
        )
        .await?,
    ))
}

/// Validate an author's proposal set and create or resubmit its review in one transaction.
///
/// # Errors
/// Rejects inaccessible or foreign-authored proposals, invalid proposal sets, stale revisions,
/// and invalid lifecycle changes. Reconciliation-required failures preserve the generated
/// candidate before returning; other failures leave the transaction uncommitted.
pub(super) async fn create_review(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    headers: HeaderMap,
    Json(request): Json<CreateReviewRequest>,
) -> Result<Json<dto::ReviewDetail>, HttpError> {
    let expected_ref = parse_ref_if_match(&headers)?;
    Ok(Json(
        service::create_review(&state.pool, &principal, expected_ref.as_deref(), request).await?,
    ))
}

/// Return reviews visible through the principal's project memberships.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_reviews(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<ListReviewsQuery>,
) -> Result<Json<dto::ReviewListResponse>, HttpError> {
    Ok(Json(
        service::list_reviews(&state.pool, &principal, query.project_id.as_deref()).await?,
    ))
}

/// Return review metadata only after checking the principal's project access.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_review(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
) -> Result<Json<dto::ReviewDetail>, HttpError> {
    Ok(Json(
        service::get_review_detail(&state.pool, &principal, &review_id).await?,
    ))
}

/// Return discussion only for a review accessible to the principal.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_review_comments(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
) -> Result<Json<dto::ReviewCommentListResponse>, HttpError> {
    Ok(Json(
        service::list_review_comments(&state.pool, &principal, &review_id).await?,
    ))
}

/// Validate access, expected revision, and final-content anchors before persisting discussion.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_review_comment(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    Json(request): Json<CreateReviewCommentRequest>,
) -> Result<Json<dto::ReviewComment>, HttpError> {
    Ok(Json(
        service::create_review_comment(&state.pool, &review_id, &principal, request).await?,
    ))
}

/// Require organization administration and project access before deciding the submitted content.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_review_decision(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    Json(request): Json<CreateReviewDecisionRequest>,
) -> Result<Json<dto::ReviewDetail>, HttpError> {
    Ok(Json(
        service::create_review_decision(&state.pool, &review_id, &principal, request).await?,
    ))
}

/// Validate access and proposal ownership before resubmitting a rejected review.
///
/// # Errors
/// Rejects inaccessible or foreign-authored proposals, invalid resubmission state, and stale
/// revisions. Required reconciliation evidence is committed before its conflict is returned;
/// other failures do not commit the submission.
pub(super) async fn create_review_submission(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateReviewSubmissionRequest>,
) -> Result<Json<dto::ReviewDetail>, HttpError> {
    let expected_ref = parse_ref_if_match(&headers)?;
    Ok(Json(
        service::create_review_submission(
            &state.pool,
            &review_id,
            &principal,
            expected_ref.as_deref(),
            request,
        )
        .await?,
    ))
}

/// Authorize publication and commit resource changes, reference advancement, and synchronization
/// events atomically.
///
/// # Errors
/// Rejects unauthorized publication, changed review or reference revisions, invalid proposal
/// state, and persistence failures. A required reconciliation candidate is committed before
/// reporting its conflict; incomplete publication is not committed.
pub(super) async fn create_review_merge(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateReviewMergeRequest>,
) -> Result<Json<dto::ReviewMergeResult>, HttpError> {
    let expected_ref = parse_ref_if_match(&headers)?;
    Ok(Json(
        service::create_review_merge(
            &state.pool,
            &review_id,
            &principal,
            expected_ref.as_deref(),
            request,
        )
        .await?,
    ))
}
