use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;

use crate::api::{
    FinalizeProjectEpisodeRequest, PreviewEpisodeSummaryRequest,
    UpdateProjectEpisodeSummaryPolicyRequest,
};
use crate::auth::AuthPrincipal;
use crate::http::{AppState, HttpError, parse_idempotency_key, parse_if_match, require_org_admin};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ListEpisodesQuery {
    after_revision: Option<i64>,
    limit: Option<i64>,
    recent: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct EvidenceQuery {
    cursor: Option<String>,
    limit: Option<i64>,
    max_bytes: Option<usize>,
}

pub(crate) async fn finalize_project_episode(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<FinalizeProjectEpisodeRequest>,
) -> Result<Json<crate::api::ProjectEpisode>, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    let idempotency_key = parse_idempotency_key(&headers)?;
    Ok(Json(
        state
            .repository
            .finalize_project_episode(&principal, &project_id, idempotency_key, request)
            .await?,
    ))
}

pub(crate) async fn list_project_episodes(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    Query(query): Query<ListEpisodesQuery>,
) -> Result<Json<crate::api::ProjectEpisodeListResponse>, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .list_project_episodes(
                &principal,
                &project_id,
                query.after_revision.unwrap_or(0),
                query.limit.unwrap_or(50),
                query.recent.unwrap_or(false),
            )
            .await?,
    ))
}

pub(crate) async fn get_episode_evidence(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, episode_id)): Path<(String, String)>,
    Query(query): Query<EvidenceQuery>,
) -> Result<Json<crate::api::EpisodeEvidencePage>, HttpError> {
    Ok(Json(
        state
            .repository
            .get_episode_evidence(
                &principal,
                &project_id,
                &episode_id,
                query.cursor.as_deref(),
                query.limit.unwrap_or(50),
                query.max_bytes.unwrap_or(65_536),
            )
            .await?,
    ))
}

pub(crate) async fn delete_project_episode(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, episode_id)): Path<(String, String)>,
) -> Result<Json<crate::api::DeleteResult>, HttpError> {
    require_org_admin(&principal)?;
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .delete_project_episode(&principal, &project_id, &episode_id)
            .await?,
    ))
}

pub(crate) async fn get_episode_summary_policy(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<crate::api::ProjectEpisodeSummaryPolicy>, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .get_episode_summary_policy(&project_id)
            .await?,
    ))
}

pub(crate) async fn update_episode_summary_policy(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateProjectEpisodeSummaryPolicyRequest>,
) -> Result<Json<crate::api::ProjectEpisodeSummaryPolicy>, HttpError> {
    require_org_admin(&principal)?;
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .update_episode_summary_policy(&principal, &project_id, expected_revision, request)
            .await?,
    ))
}

pub(crate) async fn preview_episode_summary(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, episode_id)): Path<(String, String)>,
    Json(request): Json<PreviewEpisodeSummaryRequest>,
) -> Result<Json<crate::api::EpisodeSummaryPreview>, HttpError> {
    require_org_admin(&principal)?;
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .preview_episode_summary(&project_id, &episode_id, request)
            .await?,
    ))
}

pub(crate) async fn rebuild_episode_summary(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, episode_id)): Path<(String, String)>,
) -> Result<Json<crate::api::ProjectEpisode>, HttpError> {
    require_org_admin(&principal)?;
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .rebuild_episode_summary(&principal, &project_id, &episode_id)
            .await?,
    ))
}
