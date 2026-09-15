use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::header::{CACHE_CONTROL, ETAG};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::api::{
    PersonalBundleRequest, PersonalBundleUpdateRequest, ReplaceProjectOrgSelectionRequest,
};
use crate::auth::AuthPrincipal;
use crate::http::{AppState, HttpError, parse_if_match, require_org_admin};

/// Unified Memory migration tooling: neutral, verifiable export of the org's
/// effective Memory state (memories, drafts, org
/// selections, bundles). IDs are emitted verbatim so the export doubles
/// as the old_id -> memory_id identity map.
pub(crate) async fn export_org_memory_state(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<crate::api::MemoryExport>, HttpError> {
    require_org_admin(&principal)?;
    Ok(Json(
        state
            .repository
            .export_memory_state(&principal.org_id)
            .await?,
    ))
}

pub(crate) async fn create_personal_bundle(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<PersonalBundleRequest>,
) -> Result<Json<crate::api::PersonalBundleDetail>, HttpError> {
    Ok(Json(
        state
            .repository
            .create_personal_bundle(&principal.user_id, &principal.org_id, request)
            .await?,
    ))
}

pub(crate) async fn list_personal_bundles(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<crate::api::PersonalBundleListResponse>, HttpError> {
    Ok(Json(
        state
            .repository
            .list_personal_bundles(&principal.user_id)
            .await?,
    ))
}

pub(crate) async fn get_personal_bundle(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(bundle_id): Path<String>,
) -> Result<Json<crate::api::PersonalBundleDetail>, HttpError> {
    Ok(Json(
        state
            .repository
            .get_personal_bundle(&principal.user_id, &bundle_id)
            .await?,
    ))
}

pub(crate) async fn update_personal_bundle(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(bundle_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<PersonalBundleUpdateRequest>,
) -> Result<Json<crate::api::PersonalBundleDetail>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .update_personal_bundle(
                &principal.user_id,
                &principal.org_id,
                &bundle_id,
                expected_revision,
                request,
            )
            .await?,
    ))
}

pub(crate) async fn delete_personal_bundle(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(bundle_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::api::DeleteResult>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .delete_personal_bundle(&principal.user_id, &bundle_id, expected_revision)
            .await?,
    ))
}

pub(crate) async fn list_org_memories(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<crate::api::MemoryListResponse>, HttpError> {
    Ok(Json(
        state
            .repository
            .list_org_memories(&principal.org_id)
            .await?,
    ))
}

pub(crate) async fn get_org_memory(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(memory_id): Path<String>,
) -> Result<Json<crate::api::MemoryDetail>, HttpError> {
    Ok(Json(
        state
            .repository
            .get_org_memory(&principal.org_id, &memory_id)
            .await?,
    ))
}

pub(crate) async fn list_project_memories(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<crate::api::MemoryListResponse>, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state.repository.list_project_memories(&project_id).await?,
    ))
}

pub(crate) async fn get_project_memory(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, memory_id)): Path<(String, String)>,
) -> Result<Json<crate::api::MemoryDetail>, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .get_project_memory(&project_id, &memory_id)
            .await?,
    ))
}

pub(crate) async fn get_project_org_selection(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<crate::api::ProjectOrgSelection>, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .get_project_org_selection(&project_id)
            .await?,
    ))
}

pub(crate) async fn replace_project_org_selection(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ReplaceProjectOrgSelectionRequest>,
) -> Result<Json<crate::api::ProjectOrgSelection>, HttpError> {
    state
        .repository
        .ensure_project_admin(&principal, &project_id)
        .await?;
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .replace_project_org_selection(&project_id, expected_revision, request)
            .await?,
    ))
}

pub(crate) async fn list_project_commits(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<crate::api::CommitListResponse>, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state.repository.list_project_commits(&project_id).await?,
    ))
}

#[derive(Deserialize)]
pub(crate) struct CommitStateQuery {
    local_commit_id: Option<String>,
}

pub(crate) async fn get_project_commit_state(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    Query(query): Query<CommitStateQuery>,
) -> Result<Response, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    let commit_state = state
        .repository
        .get_project_commit_state(&project_id, query.local_commit_id.as_deref())
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

pub(crate) async fn list_org_commits(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<crate::api::CommitListResponse>, HttpError> {
    Ok(Json(
        state.repository.list_org_commits(&principal.org_id).await?,
    ))
}

pub(crate) async fn get_org_commit_state(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<CommitStateQuery>,
) -> Result<Response, HttpError> {
    let commit_state = state
        .repository
        .get_org_commit_state(&principal.org_id, query.local_commit_id.as_deref())
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

pub(crate) async fn get_commit(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(commit_id): Path<String>,
) -> Result<Json<crate::api::CommitPayload>, HttpError> {
    state
        .repository
        .ensure_commit_access(&principal, &commit_id)
        .await?;
    Ok(Json(state.repository.get_commit_payload(&commit_id).await?))
}

fn ref_etag(commit_id: Option<&str>) -> String {
    format!("\"{}\"", commit_id.unwrap_or("ref-none"))
}
