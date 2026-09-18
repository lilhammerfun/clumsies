//! HTTP extraction and response construction for review resources.

use super::dto::ListReviewsQuery;
use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::review::dto::{
    CreateReviewCommentRequest, CreateReviewDecisionRequest, CreateReviewMergeRequest,
    CreateReviewRequest, CreateReviewSubmissionRequest,
};
use crate::http::{HttpError, parse_ref_if_match, require_org_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;

pub(super) async fn create_review(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    headers: HeaderMap,
    Json(request): Json<CreateReviewRequest>,
) -> Result<Json<dto::ReviewDetail>, HttpError> {
    let expected_ref = parse_ref_if_match(&headers)?;
    Ok(Json(
        service::create_review(
            &state.pool,
            &principal.user_id,
            expected_ref.as_deref(),
            request,
        )
        .await?,
    ))
}

pub(super) async fn list_reviews(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<ListReviewsQuery>,
) -> Result<Json<dto::ReviewListResponse>, HttpError> {
    Ok(Json(
        service::list_reviews(&state.pool, &principal, query.project_id.as_deref()).await?,
    ))
}

pub(super) async fn get_review(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
) -> Result<Json<dto::ReviewDetail>, HttpError> {
    service::ensure_review_member(&state.pool, &principal, &review_id).await?;
    Ok(Json(
        service::get_review_detail(&state.pool, &review_id).await?,
    ))
}

pub(super) async fn list_review_comments(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
) -> Result<Json<dto::ReviewCommentListResponse>, HttpError> {
    service::ensure_review_member(&state.pool, &principal, &review_id).await?;
    Ok(Json(
        service::list_review_comments(&state.pool, &review_id).await?,
    ))
}

pub(super) async fn create_review_comment(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    Json(request): Json<CreateReviewCommentRequest>,
) -> Result<Json<dto::ReviewComment>, HttpError> {
    service::ensure_review_member(&state.pool, &principal, &review_id).await?;
    Ok(Json(
        service::create_review_comment(&state.pool, &review_id, &principal.user_id, request)
            .await?,
    ))
}

pub(super) async fn create_review_decision(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    Json(request): Json<CreateReviewDecisionRequest>,
) -> Result<Json<dto::ReviewDetail>, HttpError> {
    require_org_admin(&principal)?;
    service::ensure_review_member(&state.pool, &principal, &review_id).await?;
    Ok(Json(
        service::create_review_decision(&state.pool, &review_id, &principal.user_id, request)
            .await?,
    ))
}

pub(super) async fn create_review_submission(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateReviewSubmissionRequest>,
) -> Result<Json<dto::ReviewDetail>, HttpError> {
    service::ensure_review_member(&state.pool, &principal, &review_id).await?;
    let expected_ref = parse_ref_if_match(&headers)?;
    Ok(Json(
        service::create_review_submission(
            &state.pool,
            &review_id,
            &principal.user_id,
            expected_ref.as_deref(),
            request,
        )
        .await?,
    ))
}

pub(super) async fn create_review_merge(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(review_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateReviewMergeRequest>,
) -> Result<Json<dto::ReviewMergeResult>, HttpError> {
    require_org_admin(&principal)?;
    service::ensure_review_member(&state.pool, &principal, &review_id).await?;
    let expected_ref = parse_ref_if_match(&headers)?;
    Ok(Json(
        service::create_review_merge(
            &state.pool,
            &review_id,
            &principal.user_id,
            expected_ref.as_deref(),
            request,
        )
        .await?,
    ))
}
