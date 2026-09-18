//! Application operations and transaction coordination for project resources.

use super::repository;
use crate::app::auth::AuthPrincipal;
use crate::app::project::dto::{
    AdminProject, AdminProjectListResponse, CreateProjectMemberRequest, CreateProjectRequest,
    Project, ProjectListResponse, ProjectMember, ProjectMemberCandidateListResponse,
    ProjectMemberListResponse, ProjectRole, UpdateProjectMemberRequest, UpdateProjectRequest,
};
use crate::app::{audit_event, organization};
use crate::dto::DeleteResult;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use crate::pagination::{admin_page, page_info};

pub async fn ensure_project_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
) -> Result<(), ServerError> {
    if repository::project_member_exists(pool, project_id, &principal.org_id, &principal.user_id)
        .await?
    {
        Ok(())
    } else {
        Err(ServerError::not_found("project", project_id))
    }
}

pub async fn ensure_project_admin(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
) -> Result<(), ServerError> {
    ensure_project_in_org(pool, &principal.org_id, project_id).await?;
    if matches!(principal.role.as_str(), "owner" | "admin") {
        return Ok(());
    }
    let member =
        repository::load_project_member(pool, &principal.org_id, project_id, &principal.user_id)
            .await?;
    if member.role == ProjectRole::Admin {
        Ok(())
    } else {
        Err(ServerError::Forbidden(
            "project administrator role required".to_owned(),
        ))
    }
}

pub async fn ensure_project_member_or_org_admin(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
) -> Result<(), ServerError> {
    if matches!(principal.role.as_str(), "owner" | "admin") {
        ensure_project_in_org(pool, &principal.org_id, project_id).await
    } else {
        ensure_project_member(pool, principal, project_id).await
    }
}

pub async fn list_project_member_candidates(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    offset: i64,
    limit: i64,
    query: Option<&str>,
) -> Result<ProjectMemberCandidateListResponse, ServerError> {
    ensure_project_admin(pool, principal, project_id).await?;
    let items =
        repository::list_project_member_candidates(pool, project_id, offset, limit + 1, query)
            .await?;
    let (items, page_info) = admin_page(items, offset, limit);
    Ok(ProjectMemberCandidateListResponse { items, page_info })
}

pub async fn list_admin_projects(
    pool: &sqlx::PgPool,
    org_id: &str,
    offset: i64,
    limit: i64,
) -> Result<AdminProjectListResponse, ServerError> {
    let items = repository::list_admin_projects(pool, org_id, offset, limit + 1).await?;
    let (items, page_info) = admin_page(items, offset, limit);
    Ok(AdminProjectListResponse { items, page_info })
}

pub async fn get_admin_project(
    pool: &sqlx::PgPool,
    org_id: &str,
    project_id: &str,
) -> Result<AdminProject, ServerError> {
    repository::load_admin_project(pool, org_id, project_id).await
}

pub async fn create_admin_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    request: CreateProjectRequest,
) -> Result<AdminProject, ServerError> {
    let name = normalize_project_name(&request.name)?;
    let description = normalize_project_description(request.description.as_deref())?;
    let project_id = prefixed_id("prj");
    let mut tx = pool.begin().await?;
    repository::ensure_project_name_available(&mut tx, &principal.org_id, &name, None).await?;
    repository::insert_project(&mut tx, &project_id, &principal.org_id, &name, &description)
        .await?;
    repository::insert_main_ref(&mut tx, &principal.org_id, &project_id).await?;
    repository::insert_selection_state(&mut tx, &project_id).await?;
    repository::insert_project_member(&mut tx, &project_id, &principal.user_id, "admin").await?;
    audit_event::service::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.project_created",
        "project",
        Some(&project_id),
    )
    .await?;
    tx.commit().await?;
    get_admin_project(pool, &principal.org_id, &project_id).await
}

pub async fn update_admin_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    expected_revision: i64,
    request: UpdateProjectRequest,
) -> Result<AdminProject, ServerError> {
    let mut tx = pool.begin().await?;
    let current = repository::lock_admin_project(&mut tx, &principal.org_id, project_id).await?;
    if current.revision != expected_revision {
        return Err(ServerError::version_conflict(
            "project",
            expected_revision,
            current.revision,
        ));
    }
    let name = match request.name {
        Some(name) => normalize_project_name(&name)?,
        None => current.name,
    };
    repository::ensure_project_name_available(&mut tx, &principal.org_id, &name, Some(project_id))
        .await?;
    let description = match request.description {
        Some(description) => normalize_project_description(Some(&description))?,
        None => current.description,
    };
    repository::update_project(&mut tx, project_id, &name, &description).await?;
    audit_event::service::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.project_updated",
        "project",
        Some(project_id),
    )
    .await?;
    tx.commit().await?;
    get_admin_project(pool, &principal.org_id, project_id).await
}

pub async fn delete_admin_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    expected_revision: i64,
) -> Result<DeleteResult, ServerError> {
    let mut tx = pool.begin().await?;
    let revision =
        repository::lock_admin_project_revision(&mut tx, &principal.org_id, project_id).await?;
    if revision != expected_revision {
        return Err(ServerError::version_conflict(
            "project",
            expected_revision,
            revision,
        ));
    }
    audit_event::service::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.project_deleted",
        "project",
        Some(project_id),
    )
    .await?;
    repository::delete_admin_project(&mut tx, &principal.org_id, project_id).await?;
    tx.commit().await?;
    Ok(DeleteResult {
        deleted: true,
        id: project_id.to_owned(),
    })
}

pub async fn list_admin_project_members(
    pool: &sqlx::PgPool,
    org_id: &str,
    project_id: &str,
    role: Option<ProjectRole>,
    offset: i64,
    limit: i64,
) -> Result<ProjectMemberListResponse, ServerError> {
    ensure_project_in_org(pool, org_id, project_id).await?;
    let items = repository::list_project_members(
        pool,
        org_id,
        project_id,
        role.map(ProjectRole::as_str),
        offset,
        limit + 1,
    )
    .await?;
    let (items, page_info) = admin_page(items, offset, limit);
    Ok(ProjectMemberListResponse { items, page_info })
}

pub async fn create_admin_project_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    request: CreateProjectMemberRequest,
) -> Result<ProjectMember, ServerError> {
    let mut tx = pool.begin().await?;
    ensure_project_in_org_tx(&mut tx, &principal.org_id, project_id).await?;
    let status = organization::service::load_user_status(&mut tx, &request.user_id).await?;
    if status == "disabled" {
        return Err(ServerError::InvalidRequest(
            "a disabled organization member cannot be added to a project".to_owned(),
        ));
    }
    if !repository::insert_project_member(
        &mut tx,
        project_id,
        &request.user_id,
        request.role.as_str(),
    )
    .await?
    {
        return Err(ServerError::already_exists(
            "project_member",
            format!("{project_id}:{}", request.user_id),
        ));
    }
    let target_id = format!("{project_id}:{}", request.user_id);
    audit_event::service::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.project_member_created",
        "project_member",
        Some(&target_id),
    )
    .await?;
    tx.commit().await?;
    repository::load_project_member(pool, &principal.org_id, project_id, &request.user_id).await
}

pub async fn update_admin_project_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    user_id: &str,
    request: UpdateProjectMemberRequest,
) -> Result<ProjectMember, ServerError> {
    let mut tx = pool.begin().await?;
    ensure_project_in_org_tx(&mut tx, &principal.org_id, project_id).await?;
    if !repository::update_project_member(&mut tx, project_id, user_id, request.role.as_str())
        .await?
    {
        return Err(ServerError::not_found(
            "project_member",
            format!("{project_id}:{user_id}"),
        ));
    }
    let target_id = format!("{project_id}:{user_id}");
    audit_event::service::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.project_member_updated",
        "project_member",
        Some(&target_id),
    )
    .await?;
    tx.commit().await?;
    repository::load_project_member(pool, &principal.org_id, project_id, user_id).await
}

pub async fn delete_admin_project_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    user_id: &str,
) -> Result<DeleteResult, ServerError> {
    let mut tx = pool.begin().await?;
    ensure_project_in_org_tx(&mut tx, &principal.org_id, project_id).await?;
    if !repository::delete_project_member(&mut tx, project_id, user_id).await? {
        return Err(ServerError::not_found(
            "project_member",
            format!("{project_id}:{user_id}"),
        ));
    }
    let target_id = format!("{project_id}:{user_id}");
    audit_event::service::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.project_member_deleted",
        "project_member",
        Some(&target_id),
    )
    .await?;
    tx.commit().await?;
    Ok(DeleteResult {
        deleted: true,
        id: user_id.to_owned(),
    })
}

pub async fn create_project(
    pool: &sqlx::PgPool,
    org_id: &str,
    name: &str,
    description: &str,
) -> Result<String, ServerError> {
    let name = normalize_project_name(name)?;
    let description = normalize_project_description(Some(description))?;
    let project_id = prefixed_id("prj");
    let mut tx = pool.begin().await?;
    repository::ensure_project_name_available(&mut tx, org_id, &name, None).await?;
    repository::insert_project(&mut tx, &project_id, org_id, &name, &description).await?;
    repository::insert_main_ref(&mut tx, org_id, &project_id).await?;
    repository::insert_selection_state(&mut tx, &project_id).await?;
    tx.commit().await?;
    Ok(project_id)
}

pub async fn create_project_from_request(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    request: CreateProjectRequest,
    idempotency_key: &str,
) -> Result<Project, ServerError> {
    let idempotency_key = normalize_idempotency_key(idempotency_key)?;
    let name = normalize_project_name(&request.name)?;
    let description = normalize_project_description(request.description.as_deref())?;
    let project_id = prefixed_id("prj");
    let mut tx = pool.begin().await?;
    let claimed = repository::claim_project_creation(
        &mut tx,
        &principal.org_id,
        &principal.user_id,
        &idempotency_key,
        &project_id,
        &name,
        &description,
    )
    .await?;
    if !claimed {
        let existing = repository::load_project_creation(
            &mut tx,
            &principal.org_id,
            &principal.user_id,
            &idempotency_key,
        )
        .await?;
        if existing.name != name || existing.description != description {
            return Err(ServerError::already_exists(
                "idempotency_key",
                idempotency_key,
            ));
        }
        tx.commit().await?;
        return get_project(pool, &existing.project_id).await;
    }
    repository::ensure_project_name_available(&mut tx, &principal.org_id, &name, None).await?;
    repository::insert_project(&mut tx, &project_id, &principal.org_id, &name, &description)
        .await?;
    repository::insert_main_ref(&mut tx, &principal.org_id, &project_id).await?;
    repository::insert_selection_state(&mut tx, &project_id).await?;
    repository::insert_project_member(&mut tx, &project_id, &principal.user_id, "admin").await?;
    audit_event::service::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "project.created",
        "project",
        Some(&project_id),
    )
    .await?;
    tx.commit().await?;
    get_project(pool, &project_id).await
}

pub async fn list_projects(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
) -> Result<ProjectListResponse, ServerError> {
    Ok(ProjectListResponse {
        items: repository::list_projects(pool, &principal.user_id, &principal.org_id).await?,
        page_info: page_info(),
    })
}

pub async fn get_project(pool: &sqlx::PgPool, project_id: &str) -> Result<Project, ServerError> {
    repository::load_project(pool, project_id).await
}

pub async fn update_project(
    pool: &sqlx::PgPool,
    project_id: &str,
    expected_version: i64,
    request: UpdateProjectRequest,
) -> Result<Project, ServerError> {
    let mut tx = pool.begin().await?;
    let current = repository::lock_project_revision(&mut tx, project_id).await?;
    if current != expected_version {
        return Err(ServerError::version_conflict(
            "project",
            expected_version,
            current,
        ));
    }
    let existing = repository::load_project_update_state(&mut tx, project_id).await?;
    let name = match request.name {
        Some(name) => normalize_project_name(&name)?,
        None => existing.name,
    };
    repository::ensure_project_name_available(&mut tx, &existing.org_id, &name, Some(project_id))
        .await?;
    let description = match request.description {
        Some(description) => normalize_project_description(Some(&description))?,
        None => existing.description,
    };
    repository::update_project(&mut tx, project_id, &name, &description).await?;
    tx.commit().await?;
    get_project(pool, project_id).await
}

pub async fn delete_project(
    pool: &sqlx::PgPool,
    project_id: &str,
    expected_version: i64,
) -> Result<DeleteResult, ServerError> {
    let mut tx = pool.begin().await?;
    let current = repository::lock_project_revision(&mut tx, project_id).await?;
    if current != expected_version {
        return Err(ServerError::version_conflict(
            "project",
            expected_version,
            current,
        ));
    }
    repository::delete_project(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(DeleteResult {
        deleted: true,
        id: project_id.to_owned(),
    })
}

async fn ensure_project_in_org(
    pool: &sqlx::PgPool,
    org_id: &str,
    project_id: &str,
) -> Result<(), ServerError> {
    if repository::project_in_org(pool, org_id, project_id).await? {
        Ok(())
    } else {
        Err(ServerError::not_found("project", project_id))
    }
}

async fn ensure_project_in_org_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: &str,
    project_id: &str,
) -> Result<(), ServerError> {
    if repository::project_in_org_tx(tx, org_id, project_id).await? {
        Ok(())
    } else {
        Err(ServerError::not_found("project", project_id))
    }
}

fn normalize_project_name(name: &str) -> Result<String, ServerError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ServerError::InvalidRequest(
            "project name must contain between 1 and 120 characters".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

fn normalize_project_description(description: Option<&str>) -> Result<String, ServerError> {
    let description = description.unwrap_or_default().trim();
    if description.chars().count() > 4_000 {
        return Err(ServerError::InvalidRequest(
            "project description must not exceed 4000 characters".to_owned(),
        ));
    }
    Ok(description.to_owned())
}

fn normalize_idempotency_key(value: &str) -> Result<String, ServerError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 200 {
        return Err(ServerError::InvalidRequest(
            "Idempotency-Key must contain between 1 and 200 bytes".to_owned(),
        ));
    }
    Ok(value.to_owned())
}

// Transaction participants share the caller-owned transaction; only the outer service commits.
pub(crate) use super::repository::list_project_refs;
