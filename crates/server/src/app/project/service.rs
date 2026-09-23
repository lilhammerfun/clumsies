//! Application operations and transaction coordination for project resources.

use super::repository;
use crate::app::auth::AuthPrincipal;
use crate::app::project::dto::{
    AdminProject, AdminProjectListResponse, CreateProjectMemberRequest, CreateProjectRequest,
    Project, ProjectListResponse, ProjectMember, ProjectMemberCandidateListResponse,
    ProjectMemberListResponse, ProjectRole, UpdateProjectMemberRequest, UpdateProjectRequest,
};
use crate::app::{audit_event, inbox, organization};
use crate::dto::DeleteResult;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use crate::pagination::{admin_page, page_info};

/// Hide projects outside the principal's organization or explicit project membership.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub(crate) async fn ensure_project_member(
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

/// Require organization administration or project-local administration within the principal's
/// organization.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub(crate) async fn ensure_project_admin(
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
    if matches!(member.role, ProjectRole::Owner | ProjectRole::Admin) {
        Ok(())
    } else {
        Err(ServerError::Forbidden(
            "project administrator role required".to_owned(),
        ))
    }
}

/// Allow organization administrators or explicit project members to read administrative project
/// details.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
async fn ensure_project_member_or_org_admin(
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

/// Return enabled organization members not yet assigned to a project the caller administers.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
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

/// Return organization-wide project administration data only to an organization administrator.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_admin_projects(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    offset: i64,
    limit: i64,
) -> Result<AdminProjectListResponse, ServerError> {
    let org_id = &principal.org_id;
    principal.require_org_admin()?;

    let items = repository::list_admin_projects(pool, org_id, offset, limit + 1).await?;
    let (items, page_info) = admin_page(items, offset, limit);
    Ok(AdminProjectListResponse { items, page_info })
}

/// Return administrative project metadata to an authorized organization administrator or project
/// member.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_admin_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
) -> Result<AdminProject, ServerError> {
    let org_id = &principal.org_id;
    ensure_project_member_or_org_admin(pool, principal, project_id).await?;

    repository::load_admin_project(pool, org_id, project_id).await
}

/// Create project metadata, its initial reference and selection, creator membership, and audit
/// record atomically.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
pub async fn create_admin_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    request: CreateProjectRequest,
) -> Result<AdminProject, ServerError> {
    principal.require_org_admin()?;

    let name = normalize_project_name(&request.name)?;
    let description = normalize_project_description(request.description.as_deref())?;
    let project_id = prefixed_id("prj");
    let mut tx = pool.begin().await?;
    repository::ensure_project_name_available(&mut tx, &principal.org_id, &name, None).await?;
    repository::insert_project(&mut tx, &project_id, &principal.org_id, &name, &description)
        .await?;
    repository::insert_main_ref(&mut tx, &principal.org_id, &project_id).await?;
    repository::insert_selection_state(&mut tx, &project_id).await?;
    repository::insert_project_member(&mut tx, &project_id, &principal.user_id, "owner").await?;
    audit_event::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.project_created",
        "project",
        Some(&project_id),
    )
    .await?;
    tx.commit().await?;
    get_admin_project(pool, principal, &project_id).await
}

/// Require project administration before applying a revision-checked update and audit record.
///
/// # Errors
/// Rejects unauthorized actors, missing projects, stale revisions, duplicate or invalid metadata,
/// and persistence failures. The metadata change and audit record share one transaction.
pub async fn update_admin_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    expected_revision: i64,
    request: UpdateProjectRequest,
) -> Result<AdminProject, ServerError> {
    ensure_project_admin(pool, principal, project_id).await?;

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
    audit_event::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.project_updated",
        "project",
        Some(project_id),
    )
    .await?;
    tx.commit().await?;
    get_admin_project(pool, principal, project_id).await
}

/// Require project administration before deleting the expected revision and recording the actor.
///
/// # Errors
/// Rejects unauthorized actors, missing projects, stale revisions, and persistence failures.
/// Deletion and its audit record commit together.
pub async fn delete_admin_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    expected_revision: i64,
) -> Result<DeleteResult, ServerError> {
    ensure_project_admin(pool, principal, project_id).await?;

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
    audit_event::insert_audit_event(
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

/// Return membership details only to an authorized project member or organization administrator.
///
/// # Errors
/// Hides inaccessible projects and propagates invalid stored roles or database failures.
pub async fn list_admin_project_members(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    role: Option<ProjectRole>,
    offset: i64,
    limit: i64,
) -> Result<ProjectMemberListResponse, ServerError> {
    let org_id = &principal.org_id;
    ensure_project_member_or_org_admin(pool, principal, project_id).await?;

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

/// Require project administration before adding an enabled organization member and recording the
/// audit event.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
pub async fn create_admin_project_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    request: CreateProjectMemberRequest,
) -> Result<ProjectMember, ServerError> {
    ensure_project_admin(pool, principal, project_id).await?;

    let mut tx = pool.begin().await?;
    ensure_project_admin_tx(&mut tx, principal, project_id).await?;
    if request.role == ProjectRole::Owner {
        return Err(ServerError::InvalidRequest(
            "project ownership cannot be assigned through member changes".to_owned(),
        ));
    }
    let status = organization::load_user_status(&mut tx, &request.user_id).await?;
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
    inbox::notify_access_change(
        &mut tx,
        principal,
        &request.user_id,
        Some(project_id),
        None,
        Some(request.role.as_str()),
    )
    .await?;
    let target_id = format!("{project_id}:{}", request.user_id);
    audit_event::insert_audit_event(
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

/// Require project administration before changing an existing member's role and recording the
/// actor.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn update_admin_project_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    user_id: &str,
    request: UpdateProjectMemberRequest,
) -> Result<ProjectMember, ServerError> {
    ensure_project_admin(pool, principal, project_id).await?;

    let mut tx = pool.begin().await?;
    ensure_project_admin_tx(&mut tx, principal, project_id).await?;
    let previous_role = repository::lock_project_member_role(&mut tx, project_id, user_id).await?;
    if previous_role == "owner" || request.role == ProjectRole::Owner {
        return Err(ServerError::InvalidRequest(
            "project ownership cannot be changed through member changes".to_owned(),
        ));
    }
    if !repository::update_project_member(&mut tx, project_id, user_id, request.role.as_str())
        .await?
    {
        return Err(ServerError::not_found(
            "project_member",
            format!("{project_id}:{user_id}"),
        ));
    }
    inbox::notify_access_change(
        &mut tx,
        principal,
        user_id,
        Some(project_id),
        Some(&previous_role),
        Some(request.role.as_str()),
    )
    .await?;
    let target_id = format!("{project_id}:{user_id}");
    audit_event::insert_audit_event(
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

/// Require project administration before removing a membership and persisting its audit event.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn delete_admin_project_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    user_id: &str,
) -> Result<DeleteResult, ServerError> {
    ensure_project_admin(pool, principal, project_id).await?;

    let mut tx = pool.begin().await?;
    ensure_project_admin_tx(&mut tx, principal, project_id).await?;
    let previous_role = repository::lock_project_member_role(&mut tx, project_id, user_id).await?;
    if previous_role == "owner" {
        return Err(ServerError::InvalidRequest(
            "the project owner cannot be removed".to_owned(),
        ));
    }
    if !repository::delete_project_member(&mut tx, project_id, user_id).await? {
        return Err(ServerError::not_found(
            "project_member",
            format!("{project_id}:{user_id}"),
        ));
    }
    inbox::notify_access_change(
        &mut tx,
        principal,
        user_id,
        Some(project_id),
        Some(&previous_role),
        None,
    )
    .await?;
    let target_id = format!("{project_id}:{user_id}");
    audit_event::insert_audit_event(
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

/// Create an administrator-owned project with its initial reference, selection state, and creator
/// membership.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
pub async fn create_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    name: &str,
    description: &str,
) -> Result<String, ServerError> {
    let org_id = &principal.org_id;
    principal.require_org_admin()?;

    let name = normalize_project_name(name)?;
    let description = normalize_project_description(Some(description))?;
    let project_id = prefixed_id("prj");
    let mut tx = pool.begin().await?;
    repository::ensure_project_name_available(&mut tx, org_id, &name, None).await?;
    repository::insert_project(&mut tx, &project_id, org_id, &name, &description).await?;
    repository::insert_main_ref(&mut tx, org_id, &project_id).await?;
    repository::insert_selection_state(&mut tx, &project_id).await?;
    repository::insert_project_member(&mut tx, &project_id, &principal.user_id, "owner").await?;
    tx.commit().await?;
    Ok(project_id)
}

/// Create a project exactly once per actor and idempotency key, rejecting reuse with different
/// metadata.
///
/// # Errors
/// Rejects invalid metadata or idempotency keys, duplicate project names, and reuse of a key with
/// different metadata; propagates persistence failures.
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
        return get_project(pool, principal, &existing.project_id).await;
    }
    repository::ensure_project_name_available(&mut tx, &principal.org_id, &name, None).await?;
    repository::insert_project(&mut tx, &project_id, &principal.org_id, &name, &description)
        .await?;
    repository::insert_main_ref(&mut tx, &principal.org_id, &project_id).await?;
    repository::insert_selection_state(&mut tx, &project_id).await?;
    repository::insert_project_member(&mut tx, &project_id, &principal.user_id, "owner").await?;
    audit_event::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "project.created",
        "project",
        Some(&project_id),
    )
    .await?;
    tx.commit().await?;
    get_project(pool, principal, &project_id).await
}

/// Return only projects assigned to the principal within its organization.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_projects(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
) -> Result<ProjectListResponse, ServerError> {
    Ok(ProjectListResponse {
        items: repository::list_projects(pool, &principal.user_id, &principal.org_id).await?,
        page_info: page_info(),
    })
}

/// Return public project metadata after enforcing explicit membership.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
) -> Result<Project, ServerError> {
    ensure_project_member(pool, principal, project_id).await?;

    repository::load_project(pool, project_id).await
}

/// Require project administration and membership before changing the expected project version.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, stale revisions or reference
/// preconditions, invalid input or lifecycle state, and propagates persistence failures.
pub async fn update_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    expected_version: i64,
    request: UpdateProjectRequest,
) -> Result<Project, ServerError> {
    ensure_project_admin(pool, principal, project_id).await?;
    ensure_project_member(pool, principal, project_id).await?;

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
    get_project(pool, principal, project_id).await
}

/// Require project administration and membership before deleting the expected project version.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, stale revisions or reference
/// preconditions, and propagates persistence failures.
pub async fn delete_project(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    expected_version: i64,
) -> Result<DeleteResult, ServerError> {
    ensure_project_admin(pool, principal, project_id).await?;
    ensure_project_member(pool, principal, project_id).await?;

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

/// Hide projects outside the specified organization before evaluating project-local privileges.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
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

/// Serialize membership changes and recheck the actor's role inside the transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
async fn ensure_project_admin_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &AuthPrincipal,
    project_id: &str,
) -> Result<(), ServerError> {
    repository::lock_admin_project_revision(tx, &principal.org_id, project_id).await?;
    if !matches!(principal.role.as_str(), "owner" | "admin") {
        let role = repository::lock_project_member_role(tx, project_id, &principal.user_id).await?;
        if !matches!(role.as_str(), "owner" | "admin") {
            return Err(ServerError::Forbidden(
                "project administrator role required".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Trim a project name and enforce its nonempty, bounded public contract.
///
/// # Errors
/// Rejects blank project names or names exceeding the supported length.
fn normalize_project_name(name: &str) -> Result<String, ServerError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ServerError::InvalidRequest(
            "project name must contain between 1 and 120 characters".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

/// Normalize optional project description text and enforce the public size limit.
///
/// # Errors
/// Rejects descriptions exceeding the supported length.
fn normalize_project_description(description: Option<&str>) -> Result<String, ServerError> {
    let description = description.unwrap_or_default().trim();
    if description.chars().count() > 4_000 {
        return Err(ServerError::InvalidRequest(
            "project description must not exceed 4000 characters".to_owned(),
        ));
    }
    Ok(description.to_owned())
}

/// Require a bounded nonblank key before recording a project-creation claim.
///
/// # Errors
/// Rejects blank keys and keys exceeding the supported length.
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

/// Return project members only after enforcing the caller's explicit project membership.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_project_members(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: &str,
    role: Option<ProjectRole>,
    offset: i64,
    limit: i64,
) -> Result<ProjectMemberListResponse, ServerError> {
    ensure_project_member(pool, principal, project_id).await?;
    list_admin_project_members(pool, principal, project_id, role, offset, limit).await
}
