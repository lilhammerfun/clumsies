//! HTTP extraction and response construction for memory resources.

use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::memory::dto::ReplaceProjectOrgSelectionRequest;
use crate::http::{HttpError, parse_if_match};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;

/// Unified Memory migration tooling: neutral, verifiable export of the org's
/// effective Memory state (memories, drafts, org
/// selections, bundles). IDs are emitted verbatim so the export doubles
/// as the old_id -> memory_id identity map.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn export_org_memory_state(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::MemoryExport>, HttpError> {
    Ok(Json(
        service::export_memory_state(&state.pool, &principal).await?,
    ))
}

/// Return active resources within the authenticated identity's organization.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_org_memories(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::MemoryListResponse>, HttpError> {
    Ok(Json(
        service::list_org_memories(&state.pool, &principal).await?,
    ))
}

/// Return one active resource within the authenticated identity's organization.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_org_memory(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(memory_id): Path<String>,
) -> Result<Json<dto::MemoryDetail>, HttpError> {
    Ok(Json(
        service::get_org_memory(&state.pool, &principal, &memory_id).await?,
    ))
}

/// Return active project resources after checking project membership.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_project_memories(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<dto::MemoryListResponse>, HttpError> {
    Ok(Json(
        service::list_project_memories(&state.pool, &principal, &project_id).await?,
    ))
}

/// Return project resource content after enforcing membership and ownership scope.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_project_memory(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, memory_id)): Path<(String, String)>,
) -> Result<Json<dto::MemoryDetail>, HttpError> {
    Ok(Json(
        service::get_project_memory(&state.pool, &principal, &project_id, &memory_id).await?,
    ))
}

/// Return a member-visible project's selected organization resources and selection revision.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_project_org_selection(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<dto::ProjectOrgSelection>, HttpError> {
    Ok(Json(
        service::get_project_org_selection(&state.pool, &principal, &project_id).await?,
    ))
}

/// Replace selected organization resources at the expected revision while coordinating active
/// drafts.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn replace_project_org_selection(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ReplaceProjectOrgSelectionRequest>,
) -> Result<Json<dto::ProjectOrgSelection>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::replace_project_org_selection(
            &state.pool,
            &principal,
            &project_id,
            expected_revision,
            request,
        )
        .await?,
    ))
}
