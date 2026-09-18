//! HTTP extraction and response construction for memory resources.

use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::memory::dto::ReplaceProjectOrgSelectionRequest;
use crate::app::project;
use crate::http::{HttpError, parse_if_match, require_org_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;

/// Unified Memory migration tooling: neutral, verifiable export of the org's
/// effective Memory state (memories, drafts, org
/// selections, bundles). IDs are emitted verbatim so the export doubles
/// as the old_id -> memory_id identity map.
pub(super) async fn export_org_memory_state(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::MemoryExport>, HttpError> {
    require_org_admin(&principal)?;
    Ok(Json(
        service::export_memory_state(&state.pool, &principal.org_id).await?,
    ))
}

pub(super) async fn list_org_memories(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::MemoryListResponse>, HttpError> {
    Ok(Json(
        service::list_org_memories(&state.pool, &principal.org_id).await?,
    ))
}

pub(super) async fn get_org_memory(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(memory_id): Path<String>,
) -> Result<Json<dto::MemoryDetail>, HttpError> {
    Ok(Json(
        service::get_org_memory(&state.pool, &principal.org_id, &memory_id).await?,
    ))
}

pub(super) async fn list_project_memories(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<dto::MemoryListResponse>, HttpError> {
    project::service::ensure_project_member(&state.pool, &principal, &project_id).await?;
    Ok(Json(
        service::list_project_memories(&state.pool, &project_id).await?,
    ))
}

pub(super) async fn get_project_memory(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, memory_id)): Path<(String, String)>,
) -> Result<Json<dto::MemoryDetail>, HttpError> {
    project::service::ensure_project_member(&state.pool, &principal, &project_id).await?;
    Ok(Json(
        service::get_project_memory(&state.pool, &project_id, &memory_id).await?,
    ))
}

pub(super) async fn get_project_org_selection(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<dto::ProjectOrgSelection>, HttpError> {
    project::service::ensure_project_member(&state.pool, &principal, &project_id).await?;
    Ok(Json(
        service::get_project_org_selection(&state.pool, &project_id).await?,
    ))
}

pub(super) async fn replace_project_org_selection(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ReplaceProjectOrgSelectionRequest>,
) -> Result<Json<dto::ProjectOrgSelection>, HttpError> {
    project::service::ensure_project_admin(&state.pool, &principal, &project_id).await?;
    project::service::ensure_project_member(&state.pool, &principal, &project_id).await?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::replace_project_org_selection(
            &state.pool,
            &project_id,
            expected_revision,
            request,
        )
        .await?,
    ))
}
