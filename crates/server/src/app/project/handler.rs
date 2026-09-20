//! HTTP extraction and response construction for project resources.

use super::dto::ListAdminProjectMembersQuery;
use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::app::project::dto::{
    CreateProjectMemberRequest, CreateProjectRequest, ProjectRole, UpdateProjectMemberRequest,
    UpdateProjectRequest,
};
use crate::error::ServerError;
use crate::http::{HttpError, parse_idempotency_key, parse_if_match};
use crate::pagination::{AdminPageQuery, AdminSearchQuery, parse_admin_page};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::header::LOCATION;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};

/// Return organization-wide project administration data only to an organization administrator.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_admin_projects(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminPageQuery>,
) -> Result<Json<dto::AdminProjectListResponse>, HttpError> {
    let page = parse_admin_page(query)?;
    Ok(Json(
        service::list_admin_projects(&state.pool, &principal, page.offset, page.limit).await?,
    ))
}

/// Create project metadata, its initial reference and selection, creator membership, and audit
/// record atomically.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_admin_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<dto::AdminProject>), HttpError> {
    Ok((
        StatusCode::CREATED,
        Json(service::create_admin_project(&state.pool, &principal, request).await?),
    ))
}

/// Return administrative project metadata to an authorized organization administrator or project
/// member.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_admin_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<dto::AdminProject>, HttpError> {
    Ok(Json(
        service::get_admin_project(&state.pool, &principal, &project_id).await?,
    ))
}

/// Require project administration before applying a revision-checked update and audit record.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn update_admin_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateProjectRequest>,
) -> Result<Json<dto::AdminProject>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::update_admin_project(
            &state.pool,
            &principal,
            &project_id,
            expected_revision,
            request,
        )
        .await?,
    ))
}

/// Require project administration before deleting the expected revision and recording the actor.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn delete_admin_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        service::delete_admin_project(&state.pool, &principal, &project_id, expected_revision)
            .await?,
    ))
}

/// Return membership details only to an authorized project member or organization administrator.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_admin_project_members(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    Query(query): Query<ListAdminProjectMembersQuery>,
) -> Result<Json<dto::ProjectMemberListResponse>, HttpError> {
    let role = parse_admin_project_role(query.role.as_deref())?;
    let page = parse_admin_page(AdminPageQuery {
        limit: query.limit,
        cursor: query.cursor,
    })?;
    Ok(Json(
        service::list_admin_project_members(
            &state.pool,
            &principal,
            &project_id,
            role,
            page.offset,
            page.limit,
        )
        .await?,
    ))
}

/// Return enabled organization members not yet assigned to a project the caller administers.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_project_member_candidates(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    Query(query): Query<AdminSearchQuery>,
) -> Result<Json<dto::ProjectMemberCandidateListResponse>, HttpError> {
    let page = parse_admin_page(query.page)?;
    Ok(Json(
        service::list_project_member_candidates(
            &state.pool,
            &principal,
            &project_id,
            page.offset,
            page.limit,
            query.q.as_deref(),
        )
        .await?,
    ))
}

/// Require project administration before adding an enabled organization member and recording the
/// audit event.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_admin_project_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    Json(request): Json<CreateProjectMemberRequest>,
) -> Result<(StatusCode, Json<dto::ProjectMember>), HttpError> {
    Ok((
        StatusCode::CREATED,
        Json(
            service::create_admin_project_member(&state.pool, &principal, &project_id, request)
                .await?,
        ),
    ))
}

/// Require project administration before changing an existing member's role and recording the
/// actor.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn update_admin_project_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, user_id)): Path<(String, String)>,
    Json(request): Json<UpdateProjectMemberRequest>,
) -> Result<Json<dto::ProjectMember>, HttpError> {
    Ok(Json(
        service::update_admin_project_member(
            &state.pool,
            &principal,
            &project_id,
            &user_id,
            request,
        )
        .await?,
    ))
}

/// Require project administration before removing a membership and persisting its audit event.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn delete_admin_project_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, user_id)): Path<(String, String)>,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    Ok(Json(
        service::delete_admin_project_member(&state.pool, &principal, &project_id, &user_id)
            .await?,
    ))
}

/// Decode the optional project-role query filter and reject unsupported values.
///
/// # Errors
/// Rejects a nonempty role filter that is not a supported project role.
fn parse_admin_project_role(role: Option<&str>) -> Result<Option<ProjectRole>, HttpError> {
    match role {
        Some("member") => Ok(Some(ProjectRole::Member)),
        Some("admin") => Ok(Some(ProjectRole::Admin)),
        Some(_) => {
            Err(ServerError::InvalidRequest("invalid project role filter".to_owned()).into())
        }
        None => Ok(None),
    }
}

/// Create an administrator-owned project with its initial reference, selection state, and creator
/// membership.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    headers: HeaderMap,
    Json(request): Json<CreateProjectRequest>,
) -> Result<
    (
        StatusCode,
        [(HeaderName, HeaderValue); 1],
        Json<dto::Project>,
    ),
    HttpError,
> {
    let idempotency_key = parse_idempotency_key(&headers)?;
    let project =
        service::create_project_from_request(&state.pool, &principal, request, idempotency_key)
            .await?;
    let location = HeaderValue::from_str(&format!("/api/v1/projects/{}", project.project_id))
        .map_err(|_| HttpError::bad_request("project URL produced an invalid Location header"))?;
    Ok((StatusCode::CREATED, [(LOCATION, location)], Json(project)))
}

/// Return only projects assigned to the principal within its organization.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_projects(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::ProjectListResponse>, HttpError> {
    Ok(Json(service::list_projects(&state.pool, &principal).await?))
}

/// Return public project metadata after enforcing explicit membership.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<dto::Project>, HttpError> {
    Ok(Json(
        service::get_project(&state.pool, &principal, &project_id).await?,
    ))
}

/// Require project administration and membership before changing the expected project version.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn update_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateProjectRequest>,
) -> Result<Json<dto::Project>, HttpError> {
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        service::update_project(
            &state.pool,
            &principal,
            &project_id,
            expected_version,
            request,
        )
        .await?,
    ))
}

/// Require project administration and membership before deleting the expected project version.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn delete_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        service::delete_project(&state.pool, &principal, &project_id, expected_version).await?,
    ))
}

/// Return project members only after enforcing the caller's explicit project membership.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_project_members(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<dto::ProjectMemberListResponse>, HttpError> {
    Ok(Json(
        service::list_project_members(&state.pool, &principal, &project_id, None, 0, 200).await?,
    ))
}
