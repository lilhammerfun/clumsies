use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::header::LOCATION;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use serde::Deserialize;

use crate::api::{
    CreateMemberRequest, CreateProjectMemberRequest, CreateProjectRequest, ProjectRole,
    UpdateAdminOrgRequest, UpdateMemberRequest, UpdateProjectMemberRequest, UpdateProjectRequest,
};
use crate::auth::AuthPrincipal;
use crate::http::{AppState, HttpError, parse_idempotency_key, parse_if_match, require_org_admin};
use crate::repository::ServerError;

pub(crate) async fn get_admin_org(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<crate::api::AdminOrg>, HttpError> {
    require_org_admin(&principal)?;
    Ok(Json(
        state.repository.get_admin_org(&principal.org_id).await?,
    ))
}

pub(crate) async fn update_admin_org(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    headers: HeaderMap,
    Json(request): Json<UpdateAdminOrgRequest>,
) -> Result<Json<crate::api::AdminOrg>, HttpError> {
    require_org_admin(&principal)?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .update_admin_org(&principal, expected_revision, request)
            .await?,
    ))
}

pub(crate) async fn list_admin_members(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminSearchQuery>,
) -> Result<Json<crate::api::MemberListResponse>, HttpError> {
    require_org_admin(&principal)?;
    let page = parse_admin_page(query.page)?;
    Ok(Json(
        state
            .repository
            .list_admin_members(page.offset, page.limit, query.q.as_deref())
            .await?,
    ))
}

pub(crate) async fn create_admin_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<CreateMemberRequest>,
) -> Result<(StatusCode, Json<crate::api::Member>), HttpError> {
    require_org_admin(&principal)?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repository
                .create_admin_member(&principal, request)
                .await?,
        ),
    ))
}

pub(crate) async fn update_admin_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateMemberRequest>,
) -> Result<Json<crate::api::Member>, HttpError> {
    require_org_admin(&principal)?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .update_admin_member(&principal, &user_id, expected_revision, request)
            .await?,
    ))
}

pub(crate) async fn delete_admin_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::api::DeleteResult>, HttpError> {
    require_org_admin(&principal)?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .delete_admin_member(&principal, &user_id, expected_revision)
            .await?,
    ))
}

pub(crate) async fn list_admin_projects(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminPageQuery>,
) -> Result<Json<crate::api::AdminProjectListResponse>, HttpError> {
    require_org_admin(&principal)?;
    let page = parse_admin_page(query)?;
    Ok(Json(
        state
            .repository
            .list_admin_projects(&principal.org_id, page.offset, page.limit)
            .await?,
    ))
}

pub(crate) async fn create_admin_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<crate::api::AdminProject>), HttpError> {
    require_org_admin(&principal)?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repository
                .create_admin_project(&principal, request)
                .await?,
        ),
    ))
}

pub(crate) async fn get_admin_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<crate::api::AdminProject>, HttpError> {
    state
        .repository
        .ensure_project_member_or_org_admin(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .get_admin_project(&principal.org_id, &project_id)
            .await?,
    ))
}

pub(crate) async fn update_admin_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateProjectRequest>,
) -> Result<Json<crate::api::AdminProject>, HttpError> {
    state
        .repository
        .ensure_project_admin(&principal, &project_id)
        .await?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .update_admin_project(&principal, &project_id, expected_revision, request)
            .await?,
    ))
}

pub(crate) async fn delete_admin_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::api::DeleteResult>, HttpError> {
    state
        .repository
        .ensure_project_admin(&principal, &project_id)
        .await?;
    let expected_revision = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .delete_admin_project(&principal, &project_id, expected_revision)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListAdminProjectMembersQuery {
    role: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
}

pub(crate) async fn list_admin_project_members(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    Query(query): Query<ListAdminProjectMembersQuery>,
) -> Result<Json<crate::api::ProjectMemberListResponse>, HttpError> {
    let role = parse_admin_project_role(query.role.as_deref())?;
    let page = parse_admin_page(AdminPageQuery {
        limit: query.limit,
        cursor: query.cursor,
    })?;
    state
        .repository
        .ensure_project_member_or_org_admin(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .list_admin_project_members(
                &principal.org_id,
                &project_id,
                role,
                page.offset,
                page.limit,
            )
            .await?,
    ))
}

pub(crate) async fn list_project_member_candidates(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    Query(query): Query<AdminSearchQuery>,
) -> Result<Json<crate::api::ProjectMemberCandidateListResponse>, HttpError> {
    let page = parse_admin_page(query.page)?;
    Ok(Json(
        state
            .repository
            .list_project_member_candidates(
                &principal,
                &project_id,
                page.offset,
                page.limit,
                query.q.as_deref(),
            )
            .await?,
    ))
}

pub(crate) async fn create_admin_project_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    Json(request): Json<CreateProjectMemberRequest>,
) -> Result<(StatusCode, Json<crate::api::ProjectMember>), HttpError> {
    state
        .repository
        .ensure_project_admin(&principal, &project_id)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repository
                .create_admin_project_member(&principal, &project_id, request)
                .await?,
        ),
    ))
}

pub(crate) async fn update_admin_project_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, user_id)): Path<(String, String)>,
    Json(request): Json<UpdateProjectMemberRequest>,
) -> Result<Json<crate::api::ProjectMember>, HttpError> {
    state
        .repository
        .ensure_project_admin(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .update_admin_project_member(&principal, &project_id, &user_id, request)
            .await?,
    ))
}

pub(crate) async fn delete_admin_project_member(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path((project_id, user_id)): Path<(String, String)>,
) -> Result<Json<crate::api::DeleteResult>, HttpError> {
    state
        .repository
        .ensure_project_admin(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .delete_admin_project_member(&principal, &project_id, &user_id)
            .await?,
    ))
}

pub(crate) async fn list_admin_tokens(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminPageQuery>,
) -> Result<Json<crate::api::AccessTokenListResponse>, HttpError> {
    require_org_admin(&principal)?;
    let page = parse_admin_page(query)?;
    Ok(Json(
        state
            .repository
            .list_admin_tokens(&principal.org_id, page.offset, page.limit)
            .await?,
    ))
}

pub(crate) async fn delete_admin_token(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(token_id): Path<String>,
) -> Result<Json<crate::api::DeleteResult>, HttpError> {
    require_org_admin(&principal)?;
    Ok(Json(
        state
            .repository
            .delete_admin_token(&principal, &token_id)
            .await?,
    ))
}

pub(crate) async fn list_admin_audit_events(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminSearchQuery>,
) -> Result<Json<crate::api::AuditEventListResponse>, HttpError> {
    require_org_admin(&principal)?;
    let page = parse_admin_page(query.page)?;
    Ok(Json(
        state
            .repository
            .list_admin_audit_events(
                &principal.org_id,
                page.offset,
                page.limit,
                query.q.as_deref(),
            )
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminSearchQuery {
    #[serde(flatten)]
    page: AdminPageQuery,
    q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminPageQuery {
    limit: Option<String>,
    cursor: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct AdminPage {
    offset: i64,
    limit: i64,
}

fn parse_admin_page(query: AdminPageQuery) -> Result<AdminPage, HttpError> {
    let limit = match query.limit {
        Some(limit) => limit.parse::<i64>().map_err(|_| {
            HttpError::from(ServerError::InvalidRequest(
                "invalid admin page limit".to_owned(),
            ))
        })?,
        None => 50,
    };
    if !(1..=200).contains(&limit) {
        return Err(ServerError::InvalidRequest(
            "admin page limit must be between 1 and 200".to_owned(),
        )
        .into());
    }
    let offset = match query.cursor {
        Some(cursor) => cursor.parse::<i64>().map_err(|_| {
            HttpError::from(ServerError::InvalidRequest(
                "invalid admin page cursor".to_owned(),
            ))
        })?,
        None => 0,
    };
    if offset < 0 {
        return Err(ServerError::InvalidRequest("invalid admin page cursor".to_owned()).into());
    }
    Ok(AdminPage { offset, limit })
}

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

pub(crate) async fn create_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    headers: HeaderMap,
    Json(request): Json<CreateProjectRequest>,
) -> Result<
    (
        StatusCode,
        [(HeaderName, HeaderValue); 1],
        Json<crate::api::Project>,
    ),
    HttpError,
> {
    let idempotency_key = parse_idempotency_key(&headers)?;
    let project = state
        .repository
        .create_project_from_request(&principal, request, idempotency_key)
        .await?;
    let location = HeaderValue::from_str(&format!("/api/v1/projects/{}", project.project_id))
        .map_err(|_| HttpError::bad_request("project URL produced an invalid Location header"))?;
    Ok((StatusCode::CREATED, [(LOCATION, location)], Json(project)))
}

pub(crate) async fn list_projects(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<crate::api::ProjectListResponse>, HttpError> {
    Ok(Json(state.repository.list_projects(&principal).await?))
}

pub(crate) async fn get_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<crate::api::Project>, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(state.repository.get_project(&project_id).await?))
}

pub(crate) async fn update_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateProjectRequest>,
) -> Result<Json<crate::api::Project>, HttpError> {
    state
        .repository
        .ensure_project_admin(&principal, &project_id)
        .await?;
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .update_project(&project_id, expected_version, request)
            .await?,
    ))
}

pub(crate) async fn delete_project(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::api::DeleteResult>, HttpError> {
    state
        .repository
        .ensure_project_admin(&principal, &project_id)
        .await?;
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    let expected_version = parse_if_match(&headers)?;
    Ok(Json(
        state
            .repository
            .delete_project(&project_id, expected_version)
            .await?,
    ))
}

pub(crate) async fn list_project_members(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(project_id): Path<String>,
) -> Result<Json<crate::api::ProjectMemberListResponse>, HttpError> {
    state
        .repository
        .ensure_project_member(&principal, &project_id)
        .await?;
    Ok(Json(
        state
            .repository
            .list_admin_project_members(&principal.org_id, &project_id, None, 0, 200)
            .await?,
    ))
}
