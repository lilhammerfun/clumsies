//! HTTP extraction and response construction for organization resources.

use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::organization::dto::{
    CreateMemberRequest, UpdateAdminOrgRequest, UpdateMemberRequest,
};
use crate::http::{HttpError, parse_if_match};
use crate::pagination::{AdminSearchQuery, parse_admin_page};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};

/// Return organization settings after verifying organization-administrator privileges.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_admin_org(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::AdminOrg>, HttpError> {
    Ok(Json(service::get_admin_org(&state.pool, &principal).await?))
}

/// Apply a revision-checked organization settings change and its audit record atomically.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn update_admin_org(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    headers: HeaderMap,
    Json(request): Json<UpdateAdminOrgRequest>,
) -> Result<Json<dto::AdminOrg>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::update_admin_org(&state.pool, &principal, expected_revision, request).await?,
    ))
}

/// Return the administrator-visible member page with filtering applied before pagination.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_admin_members(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminSearchQuery>,
) -> Result<Json<dto::MemberListResponse>, HttpError> {
    let page = parse_admin_page(query.page)?;
    Ok(Json(
        service::list_admin_members(
            &state.pool,
            &principal,
            page.offset,
            page.limit,
            query.q.as_deref(),
        )
        .await?,
    ))
}

/// Validate an invitation's role and email policy, then persist the member and audit event.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_admin_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<CreateMemberRequest>,
) -> Result<(StatusCode, Json<dto::Member>), HttpError> {
    Ok((
        StatusCode::CREATED,
        Json(service::create_admin_member(&state.pool, &principal, request).await?),
    ))
}

/// Apply a member change while protecting the caller and the last active owner.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn update_admin_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateMemberRequest>,
) -> Result<Json<dto::Member>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::update_admin_member(
            &state.pool,
            &principal,
            &user_id,
            expected_revision,
            request,
        )
        .await?,
    ))
}

/// Disable a member and revoke its credentials while preserving an active organization owner.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn delete_admin_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::delete_admin_member(&state.pool, &principal, &user_id, expected_revision).await?,
    ))
}
