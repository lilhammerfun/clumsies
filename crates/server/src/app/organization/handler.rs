//! HTTP extraction and response construction for organization resources.

use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::organization::dto::{
    CreateMemberRequest, UpdateAdminOrgRequest, UpdateMemberRequest,
};
use crate::http::{HttpError, parse_if_match, require_org_admin};
use crate::pagination::{AdminSearchQuery, parse_admin_page};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};

pub(super) async fn get_admin_org(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::AdminOrg>, HttpError> {
    require_org_admin(&principal)?;
    Ok(Json(
        service::get_admin_org(&state.pool, &principal.org_id).await?,
    ))
}

pub(super) async fn update_admin_org(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    headers: HeaderMap,
    Json(request): Json<UpdateAdminOrgRequest>,
) -> Result<Json<dto::AdminOrg>, HttpError> {
    require_org_admin(&principal)?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::update_admin_org(&state.pool, &principal, expected_revision, request).await?,
    ))
}

pub(super) async fn list_admin_members(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminSearchQuery>,
) -> Result<Json<dto::MemberListResponse>, HttpError> {
    require_org_admin(&principal)?;
    let page = parse_admin_page(query.page)?;
    Ok(Json(
        service::list_admin_members(&state.pool, page.offset, page.limit, query.q.as_deref())
            .await?,
    ))
}

pub(super) async fn create_admin_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<CreateMemberRequest>,
) -> Result<(StatusCode, Json<dto::Member>), HttpError> {
    require_org_admin(&principal)?;
    Ok((
        StatusCode::CREATED,
        Json(service::create_admin_member(&state.pool, &principal, request).await?),
    ))
}

pub(super) async fn update_admin_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateMemberRequest>,
) -> Result<Json<dto::Member>, HttpError> {
    require_org_admin(&principal)?;
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

pub(super) async fn delete_admin_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    require_org_admin(&principal)?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::delete_admin_member(&state.pool, &principal, &user_id, expected_revision).await?,
    ))
}
