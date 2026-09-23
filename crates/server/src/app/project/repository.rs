//! SQL queries and persistence for project resources.

use super::model::{LockedProject, ProjectCreation, ProjectUpdateState, project_role};
use crate::app::organization::dto::UserRef;
use crate::app::project::dto::{AdminProject, Project, ProjectMember, ProjectRef};
use crate::error::ServerError;
use crate::identity::prefixed_id;
use sqlx::{FromRow, PgPool, Postgres, Row, Transaction};

/// Read the requesting user's accessible projects and project-local roles.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_project_refs(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    user_id: &str,
) -> Result<Vec<ProjectRef>, ServerError> {
    let rows = sqlx::query(
        "SELECT p.project_id, p.name, m.role
         FROM projects p
         JOIN project_members m ON m.project_id = p.project_id
         WHERE p.org_id = $1 AND m.user_id = $2
         ORDER BY p.created_at",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(ProjectRef {
                project_id: row.try_get("project_id")?,
                name: row.try_get("name")?,
                role: project_role(row.try_get::<String, _>("role")?.as_str())?,
            })
        })
        .collect()
}

/// Check whether a user has an explicit membership in the project.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn project_member_exists(
    pool: &PgPool,
    project_id: &str,
    org_id: &str,
    user_id: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM projects p
            JOIN project_members m ON m.project_id = p.project_id
            WHERE p.project_id = $1 AND p.org_id = $2 AND m.user_id = $3
         )",
    )
    .bind(project_id)
    .bind(org_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?)
}

/// Read enabled organization members not already assigned to the project.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_project_member_candidates(
    pool: &PgPool,
    project_id: &str,
    offset: i64,
    limit: i64,
    query: Option<&str>,
) -> Result<Vec<UserRef>, ServerError> {
    let rows = sqlx::query(
        "SELECT u.user_id, u.email, u.display_name, u.avatar_url, u.role
         FROM users u
         WHERE u.status != 'disabled'
           AND NOT EXISTS (
               SELECT 1 FROM project_members m WHERE m.project_id = $1 AND m.user_id = u.user_id
           )
           AND ($4::text IS NULL
               OR strpos(lower(concat_ws(' ', u.email, u.display_name)), lower($4)) > 0)
         ORDER BY u.created_at, u.user_id
         LIMIT $2 OFFSET $3",
    )
    .bind(project_id)
    .bind(limit)
    .bind(offset)
    .bind(query)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| UserRef::from_row(row).map_err(ServerError::from))
        .collect()
}

/// Read organization project metadata with filtering before the page limit.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_admin_projects(
    pool: &PgPool,
    org_id: &str,
    offset: i64,
    limit: i64,
) -> Result<Vec<AdminProject>, ServerError> {
    let rows = sqlx::query(
        "SELECT p.project_id, p.name, p.description, p.revision,
                p.created_at, p.updated_at,
                COUNT(m.user_id)::BIGINT AS member_count
         FROM projects p
         LEFT JOIN project_members m ON m.project_id = p.project_id
         WHERE p.org_id = $1
         GROUP BY p.project_id
         ORDER BY p.updated_at DESC, p.project_id
         LIMIT $2 OFFSET $3",
    )
    .bind(org_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(admin_project_from_row).collect()
}

/// Read administrative project metadata and its membership count.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_admin_project(
    pool: &PgPool,
    org_id: &str,
    project_id: &str,
) -> Result<AdminProject, ServerError> {
    let row = sqlx::query(
        "SELECT p.project_id, p.name, p.description, p.revision,
                p.created_at, p.updated_at,
                COUNT(m.user_id)::BIGINT AS member_count
         FROM projects p
         LEFT JOIN project_members m ON m.project_id = p.project_id
         WHERE p.org_id = $1 AND p.project_id = $2
         GROUP BY p.project_id",
    )
    .bind(org_id)
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ServerError::not_found("project", project_id))?;
    admin_project_from_row(&row)
}

/// Lock project metadata before a revision-checked administrative update.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn lock_admin_project(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    project_id: &str,
) -> Result<LockedProject, ServerError> {
    let row = sqlx::query(
        "SELECT name, description, revision
         FROM projects
         WHERE project_id = $1 AND org_id = $2
         FOR UPDATE",
    )
    .bind(project_id)
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("project", project_id))?;
    Ok(LockedProject {
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        revision: row.try_get("revision")?,
    })
}

/// Lock and read a project's current concurrency revision before deletion.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn lock_admin_project_revision(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    project_id: &str,
) -> Result<i64, ServerError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM projects
         WHERE project_id = $1 AND org_id = $2
         FOR UPDATE",
    )
    .bind(project_id)
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("project", project_id))
}

/// Delete project metadata within the caller's administrative transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn delete_admin_project(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    project_id: &str,
) -> Result<(), ServerError> {
    sqlx::query("DELETE FROM projects WHERE project_id = $1 AND org_id = $2")
        .bind(project_id)
        .bind(org_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Check project ownership without disclosing metadata from another organization.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn project_in_org(
    pool: &PgPool,
    org_id: &str,
    project_id: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM projects WHERE project_id = $1 AND org_id = $2)",
    )
    .bind(project_id)
    .bind(org_id)
    .fetch_one(pool)
    .await?)
}

/// Read joined member identities and project roles with filtering before pagination.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_project_members(
    pool: &PgPool,
    org_id: &str,
    project_id: &str,
    role: Option<&str>,
    offset: i64,
    limit: i64,
) -> Result<Vec<ProjectMember>, ServerError> {
    let rows = sqlx::query(
        "SELECT p.project_id, u.user_id, u.email, u.display_name, u.avatar_url,
                u.role AS org_role, m.role AS project_role, m.joined_at
         FROM project_members m
         JOIN projects p ON p.project_id = m.project_id
         JOIN users u ON u.user_id = m.user_id
         WHERE p.project_id = $1
           AND p.org_id = $2
           AND ($3::TEXT IS NULL OR m.role = $3)
         ORDER BY m.joined_at, u.user_id
         LIMIT $4 OFFSET $5",
    )
    .bind(project_id)
    .bind(org_id)
    .bind(role)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(project_member_from_row).collect()
}

/// Persist a project-local role for an already validated organization member.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn insert_project_member(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    user_id: &str,
    role: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query(
        "INSERT INTO project_members (project_id, user_id, role)
         VALUES ($1, $2, $3)
         ON CONFLICT (project_id, user_id) DO NOTHING",
    )
    .bind(project_id)
    .bind(user_id)
    .bind(role)
    .execute(&mut **tx)
    .await?
    .rows_affected()
        == 1)
}

/// Read one project member's public identity and local privileges.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_project_member(
    pool: &PgPool,
    org_id: &str,
    project_id: &str,
    user_id: &str,
) -> Result<ProjectMember, ServerError> {
    let row = sqlx::query(
        "SELECT p.project_id, u.user_id, u.email, u.display_name, u.avatar_url,
                u.role AS org_role, m.role AS project_role, m.joined_at
         FROM project_members m
         JOIN projects p ON p.project_id = m.project_id
         JOIN users u ON u.user_id = m.user_id
         WHERE p.project_id = $1 AND p.org_id = $2 AND u.user_id = $3",
    )
    .bind(project_id)
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ServerError::not_found("project_member", format!("{project_id}:{user_id}")))?;
    project_member_from_row(&row)
}

/// Lock a membership so a notification records the role immediately preceding its change.
///
/// # Errors
/// Returns not found for a missing member and propagates database failures.
pub(crate) async fn lock_project_member_role(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    user_id: &str,
) -> Result<String, ServerError> {
    sqlx::query_scalar(
        "SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("project_member", format!("{project_id}:{user_id}")))
}

/// Replace an existing membership's project-local role.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn update_project_member(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    user_id: &str,
    role: &str,
) -> Result<bool, ServerError> {
    Ok(
        sqlx::query("UPDATE project_members SET role = $3 WHERE project_id = $1 AND user_id = $2")
            .bind(project_id)
            .bind(user_id)
            .bind(role)
            .execute(&mut **tx)
            .await?
            .rows_affected()
            == 1,
    )
}

/// Delete an existing project membership within the caller's transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn delete_project_member(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    user_id: &str,
) -> Result<bool, ServerError> {
    Ok(
        sqlx::query("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2")
            .bind(project_id)
            .bind(user_id)
            .execute(&mut **tx)
            .await?
            .rows_affected()
            == 1,
    )
}

/// Reject a duplicate project name before inserting or renaming project metadata.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn ensure_project_name_available(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    name: &str,
    excluded_project_id: Option<&str>,
) -> Result<(), ServerError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(org_id)
        .fetch_one(&mut **tx)
        .await?;
    let existing = sqlx::query_scalar::<_, String>(
        "SELECT project_id
         FROM projects
         WHERE org_id = $1
           AND lower(name) = lower($2)
           AND ($3::TEXT IS NULL OR project_id <> $3)
         LIMIT 1",
    )
    .bind(org_id)
    .bind(name)
    .bind(excluded_project_id)
    .fetch_optional(&mut **tx)
    .await?;
    if existing.is_some() {
        return Err(ServerError::already_exists("project_name", name));
    }
    Ok(())
}

/// Persist normalized project metadata and its initial version.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn insert_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    org_id: &str,
    name: &str,
    description: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO projects (project_id, org_id, name, description)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(project_id)
    .bind(org_id)
    .bind(name)
    .bind(description)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Create a project's initial empty main snapshot reference.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn insert_main_ref(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    project_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO refs (ref_id, ref_name, scope, org_id, project_id)
         VALUES ($1, 'refs/heads/main', 'project', $2, $3)",
    )
    .bind(prefixed_id("ref"))
    .bind(org_id)
    .bind(project_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Create the initial concurrency revision for a project's organization selection.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn insert_selection_state(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<(), ServerError> {
    sqlx::query("INSERT INTO project_org_selection_states (project_id) VALUES ($1)")
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Record an actor's idempotency key and payload fingerprint exactly once.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn claim_project_creation(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    user_id: &str,
    idempotency_key: &str,
    project_id: &str,
    request_name: &str,
    request_description: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query(
        "INSERT INTO project_creation_requests (
             org_id, user_id, idempotency_key, project_id,
             request_name, request_description
         )
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (org_id, user_id, idempotency_key) DO NOTHING",
    )
    .bind(org_id)
    .bind(user_id)
    .bind(idempotency_key)
    .bind(project_id)
    .bind(request_name)
    .bind(request_description)
    .execute(&mut **tx)
    .await?
    .rows_affected()
        == 1)
}

/// Read a previous creation claim for idempotent response replay.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn load_project_creation(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    user_id: &str,
    idempotency_key: &str,
) -> Result<ProjectCreation, ServerError> {
    let row = sqlx::query(
        "SELECT project_id, request_name, request_description
         FROM project_creation_requests
         WHERE org_id = $1 AND user_id = $2 AND idempotency_key = $3",
    )
    .bind(org_id)
    .bind(user_id)
    .bind(idempotency_key)
    .fetch_one(&mut **tx)
    .await?;
    Ok(ProjectCreation {
        project_id: row.try_get("project_id")?,
        name: row.try_get("request_name")?,
        description: row.try_get("request_description")?,
    })
}

/// Read public projects joined to the supplied user's explicit memberships.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_projects(
    pool: &PgPool,
    user_id: &str,
    org_id: &str,
) -> Result<Vec<Project>, ServerError> {
    let rows = sqlx::query(
        "SELECT p.project_id, p.name, p.description, p.revision, p.created_at, p.updated_at
         FROM projects p
         JOIN project_members m ON m.project_id = p.project_id
         WHERE m.user_id = $1 AND p.org_id = $2
         ORDER BY p.updated_at DESC
         LIMIT 200",
    )
    .bind(user_id)
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(project_from_row).collect()
}

/// Read required public project metadata.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_project(pool: &PgPool, project_id: &str) -> Result<Project, ServerError> {
    let row = sqlx::query(
        "SELECT project_id, name, description, revision, created_at, updated_at
         FROM projects
         WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ServerError::not_found("project", project_id))?;
    project_from_row(&row)
}

/// Lock a project's current revision before a conditional mutation.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn lock_project_revision(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<i64, ServerError> {
    sqlx::query_scalar::<_, i64>("SELECT revision FROM projects WHERE project_id = $1 FOR UPDATE")
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("project", project_id))
}

/// Read project ownership and normalized fields needed to apply an update.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn load_project_update_state(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<ProjectUpdateState, ServerError> {
    let row = sqlx::query(
        "SELECT org_id, name, description
         FROM projects
         WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(ProjectUpdateState {
        org_id: row.try_get("org_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
    })
}

/// Persist normalized project metadata and advance its version.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn update_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    name: &str,
    description: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE projects
         SET name = $2, description = $3, revision = revision + 1, updated_at = now()
         WHERE project_id = $1",
    )
    .bind(project_id)
    .bind(name)
    .bind(description)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Delete project metadata inside the caller's transaction.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn delete_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<(), ServerError> {
    sqlx::query("DELETE FROM projects WHERE project_id = $1")
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Decode administrative project metadata and aggregate membership count.
///
/// # Errors
/// Propagates database access and row-decoding failures.
fn admin_project_from_row(row: &sqlx::postgres::PgRow) -> Result<AdminProject, ServerError> {
    Ok(AdminProject {
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        member_count: row.try_get("member_count")?,
        revision: row.try_get("revision")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Decode public member identity and project-local role.
///
/// # Errors
/// Propagates database access and row-decoding failures.
fn project_member_from_row(row: &sqlx::postgres::PgRow) -> Result<ProjectMember, ServerError> {
    Ok(ProjectMember {
        project_id: row.try_get("project_id")?,
        user: UserRef {
            user_id: row.try_get("user_id")?,
            email: row.try_get("email")?,
            display_name: row.try_get("display_name")?,
            avatar_url: row.try_get("avatar_url")?,
            role: row.try_get("org_role")?,
        },
        role: project_role(row.try_get::<String, _>("project_role")?.as_str())?,
        joined_at: row.try_get("joined_at")?,
    })
}

/// Decode public project metadata and its concurrency revision.
///
/// # Errors
/// Propagates database access and row-decoding failures.
fn project_from_row(row: &sqlx::postgres::PgRow) -> Result<Project, ServerError> {
    Ok(Project {
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        revision: row.try_get("revision")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
