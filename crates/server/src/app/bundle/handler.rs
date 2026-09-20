//! HTTP extraction and response construction for bundle resources.

use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::bundle::dto::{PersonalBundleRequest, PersonalBundleUpdateRequest};
use crate::http::{HttpError, parse_if_match};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;

/// Create an owned Memory collection and its validated resource selection atomically.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_personal_bundle(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<PersonalBundleRequest>,
) -> Result<Json<dto::PersonalBundleDetail>, HttpError> {
    Ok(Json(
        service::create_personal_bundle(
            &state.pool,
            &principal.user_id,
            &principal.org_id,
            request,
        )
        .await?,
    ))
}

/// Return collections belonging to the authenticated owner.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_personal_bundles(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::PersonalBundleListResponse>, HttpError> {
    Ok(Json(
        service::list_personal_bundles(&state.pool, &principal.user_id).await?,
    ))
}

/// Require collection ownership before assembling its active selected memories.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_personal_bundle(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(bundle_id): Path<String>,
) -> Result<Json<dto::PersonalBundleDetail>, HttpError> {
    Ok(Json(
        service::get_personal_bundle(&state.pool, &principal.user_id, &bundle_id).await?,
    ))
}

/// Apply owner-authorized metadata and selection changes at the expected revision.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn update_personal_bundle(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(bundle_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<PersonalBundleUpdateRequest>,
) -> Result<Json<dto::PersonalBundleDetail>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::update_personal_bundle(
            &state.pool,
            &principal.user_id,
            &principal.org_id,
            &bundle_id,
            expected_revision,
            request,
        )
        .await?,
    ))
}

/// Delete an owned collection only when its revision matches the caller's precondition.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn delete_personal_bundle(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(bundle_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::delete_personal_bundle(
            &state.pool,
            &principal.user_id,
            &bundle_id,
            expected_revision,
        )
        .await?,
    ))
}
