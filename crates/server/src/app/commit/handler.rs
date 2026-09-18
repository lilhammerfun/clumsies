//! HTTP extraction and response construction for commit resources.

use super::dto::CommitStateQuery;
use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::project;
use crate::http::HttpError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, ETAG};
use axum::response::{IntoResponse, Response};

pub(super) async fn list_project_commits(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<dto::CommitListResponse>, HttpError> {
    project::service::ensure_project_member(&state.pool, &principal, &project_id).await?;
    Ok(Json(
        service::list_project_commits(&state.pool, &project_id).await?,
    ))
}

pub(super) async fn get_project_commit_state(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    Query(query): Query<CommitStateQuery>,
) -> Result<Response, HttpError> {
    project::service::ensure_project_member(&state.pool, &principal, &project_id).await?;
    let commit_state = service::get_project_commit_state(
        &state.pool,
        &project_id,
        query.local_commit_id.as_deref(),
    )
    .await?;
    let mut response = Json(commit_state.clone()).into_response();
    let etag = ref_etag(commit_state.reference.commit_id.as_deref());
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(&etag)
            .map_err(|_| HttpError::bad_request("ref produced an invalid ETag"))?,
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-transform"));
    Ok(response)
}

pub(super) async fn list_org_commits(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::CommitListResponse>, HttpError> {
    Ok(Json(
        service::list_org_commits(&state.pool, &principal.org_id).await?,
    ))
}

pub(super) async fn get_org_commit_state(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<CommitStateQuery>,
) -> Result<Response, HttpError> {
    let commit_state = service::get_org_commit_state(
        &state.pool,
        &principal.org_id,
        query.local_commit_id.as_deref(),
    )
    .await?;
    let mut response = Json(commit_state.clone()).into_response();
    let etag = ref_etag(commit_state.reference.commit_id.as_deref());
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(&etag)
            .map_err(|_| HttpError::bad_request("ref produced an invalid ETag"))?,
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-transform"));
    Ok(response)
}

pub(super) async fn get_commit(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(commit_id): Path<String>,
) -> Result<Json<dto::CommitPayload>, HttpError> {
    service::ensure_commit_access(&state.pool, &principal, &commit_id).await?;
    Ok(Json(
        service::get_commit_payload(&state.pool, &commit_id).await?,
    ))
}

fn ref_etag(commit_id: Option<&str>) -> String {
    format!("\"{}\"", commit_id.unwrap_or("ref-none"))
}
