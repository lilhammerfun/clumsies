use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::*;
use crate::repository::ServerError;

pub(super) struct LockedAdminOrg {
    pub(super) name: String,
    pub(super) allowed_email_domains: Vec<String>,
    pub(super) revision: i64,
}

pub(super) struct LockedMember {
    pub(super) role: String,
    pub(super) status: String,
    pub(super) revision: i64,
}

pub(super) struct LockedProject {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) revision: i64,
}

pub(super) struct ProjectCreation {
    pub(super) project_id: String,
    pub(super) name: String,
    pub(super) description: String,
}

pub(super) struct ProjectUpdateState {
    pub(super) org_id: String,
    pub(super) name: String,
    pub(super) description: String,
}

pub(super) async fn load_user_ref(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
) -> Result<UserRef, ServerError> {
    let row = sqlx::query(
        "SELECT user_id, email, display_name, avatar_url, role
         FROM users
         WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("user", user_id))?;
    user_ref_from_row(&row)
}

pub(super) async fn load_org_ref(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
) -> Result<OrgRef, ServerError> {
    let row = sqlx::query("SELECT org_id, name FROM orgs WHERE org_id = $1")
        .bind(org_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("org", org_id))?;
    Ok(OrgRef {
        org_id: row.try_get("org_id")?,
        name: row.try_get("name")?,
    })
}

pub(super) async fn list_project_refs(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    user_id: &str,
) -> Result<Vec<ProjectRef>, ServerError> {
    let rows = sqlx::query(
        "SELECT p.project_id, p.name
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
            })
        })
        .collect()
}

pub(super) async fn project_member_exists(
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

pub(super) async fn load_admin_org(pool: &PgPool, org_id: &str) -> Result<AdminOrg, ServerError> {
    let row = sqlx::query(
        "SELECT org_id, name, allowed_email_domains, revision, updated_at
         FROM orgs WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ServerError::not_found("org", org_id))?;
    admin_org_from_row(&row)
}

pub(super) async fn lock_admin_org(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
) -> Result<LockedAdminOrg, ServerError> {
    let row = sqlx::query(
        "SELECT name, allowed_email_domains, revision
         FROM orgs WHERE org_id = $1 FOR UPDATE",
    )
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("org", org_id))?;
    Ok(LockedAdminOrg {
        name: row.try_get("name")?,
        allowed_email_domains: row.try_get("allowed_email_domains")?,
        revision: row.try_get("revision")?,
    })
}

pub(super) async fn update_admin_org(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    name: &str,
    allowed_email_domains: &[String],
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE orgs
         SET name = $2, allowed_email_domains = $3, revision = revision + 1, updated_at = now()
         WHERE org_id = $1",
    )
    .bind(org_id)
    .bind(name)
    .bind(allowed_email_domains)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn list_admin_members(
    pool: &PgPool,
    offset: i64,
    limit: i64,
    query: Option<&str>,
) -> Result<Vec<Member>, ServerError> {
    let rows = sqlx::query(
        "SELECT u.user_id, u.email, u.display_name, u.role, u.status, u.revision,
                EXISTS (
                    SELECT 1 FROM external_identities i WHERE i.user_id = u.user_id
                ) AS external_identity_bound
         FROM users u
         WHERE $3::text IS NULL
            OR strpos(lower(concat_ws(' ', u.email, u.display_name)), lower($3)) > 0
         ORDER BY u.created_at, u.user_id
         LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .bind(query)
    .fetch_all(pool)
    .await?;
    rows.iter().map(member_from_row).collect()
}

pub(super) async fn load_allowed_email_domains(
    pool: &PgPool,
    org_id: &str,
) -> Result<Vec<String>, ServerError> {
    sqlx::query_scalar::<_, Vec<String>>("SELECT allowed_email_domains FROM orgs WHERE org_id = $1")
        .bind(org_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ServerError::not_found("org", org_id))
}

pub(super) async fn member_email_exists(pool: &PgPool, email: &str) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM users WHERE lower(email) = lower($1))",
    )
    .bind(email)
    .fetch_one(pool)
    .await?)
}

pub(super) async fn insert_member(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
    email: &str,
    role: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO users (user_id, email, role, status)
         VALUES ($1, $2, $3, 'invited')",
    )
    .bind(user_id)
    .bind(email)
    .bind(role)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn lock_member(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
) -> Result<LockedMember, ServerError> {
    let row = sqlx::query("SELECT role, status, revision FROM users WHERE user_id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("user", user_id))?;
    Ok(LockedMember {
        role: row.try_get("role")?,
        status: row.try_get("status")?,
        revision: row.try_get("revision")?,
    })
}

pub(super) async fn active_owner_count(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<i64, ServerError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM users WHERE role = 'owner' AND status = 'active'",
    )
    .fetch_one(&mut **tx)
    .await?)
}

pub(super) async fn update_member(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
    role: &str,
    status: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE users
         SET role = $2, status = $3, revision = revision + 1, updated_at = now()
         WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(role)
    .bind(status)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn revoke_user_sessions(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    user_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE access_tokens t SET revoked_at = now()
         FROM auth_sessions s
         WHERE t.session_id = s.session_id AND s.org_id = $1 AND s.user_id = $2",
    )
    .bind(org_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE auth_sessions SET revoked_at = now()
         WHERE org_id = $1 AND user_id = $2",
    )
    .bind(org_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn load_member(pool: &PgPool, user_id: &str) -> Result<Member, ServerError> {
    let row = sqlx::query(
        "SELECT u.user_id, u.email, u.display_name, u.role, u.status, u.revision,
                EXISTS (
                    SELECT 1 FROM external_identities i WHERE i.user_id = u.user_id
                ) AS external_identity_bound
         FROM users u WHERE u.user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ServerError::not_found("user", user_id))?;
    member_from_row(&row)
}

pub(super) async fn list_admin_projects(
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

pub(super) async fn load_admin_project(
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

pub(super) async fn lock_admin_project(
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

pub(super) async fn lock_admin_project_revision(
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

pub(super) async fn delete_admin_project(
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

pub(super) async fn project_in_org(
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

pub(super) async fn project_in_org_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    project_id: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM projects WHERE project_id = $1 AND org_id = $2)",
    )
    .bind(project_id)
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await?)
}

pub(super) async fn list_project_members(
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

pub(super) async fn load_user_status(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
) -> Result<String, ServerError> {
    sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("user", user_id))
}

pub(super) async fn insert_project_member(
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

pub(super) async fn load_project_member(
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

pub(super) async fn update_project_member(
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

pub(super) async fn delete_project_member(
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

pub(super) async fn list_access_tokens(
    pool: &PgPool,
    org_id: &str,
    offset: i64,
    limit: i64,
) -> Result<Vec<AccessTokenMeta>, ServerError> {
    let rows = sqlx::query(
        "SELECT t.token_id, t.user_id, t.kind, t.revoked_at, t.expires_at, t.created_at
         FROM access_tokens t
         JOIN auth_sessions s ON s.session_id = t.session_id
         WHERE s.org_id = $1
         ORDER BY t.created_at DESC, t.token_id
         LIMIT $2 OFFSET $3",
    )
    .bind(org_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(access_token_meta_from_row).collect()
}

pub(super) async fn access_token_exists(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    token_id: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM access_tokens t
            JOIN auth_sessions s ON s.session_id = t.session_id
            WHERE t.token_id = $1 AND s.org_id = $2
         )",
    )
    .bind(token_id)
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await?)
}

pub(super) async fn revoke_access_token(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    token_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE access_tokens t SET revoked_at = now()
         FROM auth_sessions s
         WHERE t.session_id = s.session_id AND t.token_id = $1 AND s.org_id = $2",
    )
    .bind(token_id)
    .bind(org_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn list_audit_events(
    pool: &PgPool,
    org_id: &str,
    offset: i64,
    limit: i64,
    query: Option<&str>,
) -> Result<Vec<AuditEvent>, ServerError> {
    let rows = sqlx::query(
        "WITH events AS (
            SELECT e.event_id, e.actor_user_id, actor.display_name AS actor_display_name,
                   actor.email AS actor_email, e.action, e.target_type, e.target_id, e.created_at,
                   CASE e.target_type
                       WHEN 'org' THEN target_org.name
                       WHEN 'project' THEN target_project.name
                       WHEN 'user' THEN COALESCE(target_user.display_name, target_user.email)
                       WHEN 'project_member' THEN target_project.name || ' · ' ||
                           COALESCE(target_user.display_name, target_user.email)
                   END AS target_display_name
            FROM audit_events e
            LEFT JOIN users actor ON actor.user_id = e.actor_user_id
            LEFT JOIN orgs target_org ON e.target_type = 'org'
                AND target_org.org_id = e.target_id AND target_org.org_id = e.org_id
            LEFT JOIN projects target_project ON target_project.org_id = e.org_id
                AND target_project.project_id = CASE e.target_type
                    WHEN 'project' THEN e.target_id
                    WHEN 'project_member' THEN split_part(e.target_id, ':', 1)
                END
            LEFT JOIN users target_user ON target_user.user_id = CASE e.target_type
                WHEN 'user' THEN e.target_id
                WHEN 'project_member' THEN split_part(e.target_id, ':', 2)
            END
            WHERE e.org_id = $1
         )
         SELECT * FROM events
         WHERE $4::text IS NULL OR strpos(lower(concat_ws(' ', action,
             actor_display_name, actor_email, target_display_name)), lower($4)) > 0
         ORDER BY created_at DESC, event_id
         LIMIT $2 OFFSET $3",
    )
    .bind(org_id)
    .bind(limit)
    .bind(offset)
    .bind(query)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(AuditEvent {
                event_id: row.try_get("event_id")?,
                actor_user_id: row.try_get("actor_user_id")?,
                actor_display_name: row.try_get("actor_display_name")?,
                actor_email: row.try_get("actor_email")?,
                action: row.try_get("action")?,
                target_type: row.try_get("target_type")?,
                target_id: row.try_get("target_id")?,
                target_display_name: row.try_get("target_display_name")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

pub(super) async fn insert_audit_event(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    actor_user_id: Option<&str>,
    action: &str,
    target_type: &str,
    target_id: Option<&str>,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO audit_events (
            event_id, org_id, actor_user_id, action, target_type, target_id
         ) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(prefixed_id("evt"))
    .bind(org_id)
    .bind(actor_user_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn ensure_project_name_available(
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

pub(super) async fn insert_project(
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

pub(super) async fn insert_main_ref(
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

pub(super) async fn insert_selection_state(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<(), ServerError> {
    sqlx::query("INSERT INTO project_org_selection_states (project_id) VALUES ($1)")
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(super) async fn claim_project_creation(
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

pub(super) async fn load_project_creation(
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

pub(super) async fn list_projects(
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

pub(super) async fn load_project(pool: &PgPool, project_id: &str) -> Result<Project, ServerError> {
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

pub(super) async fn lock_project_revision(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<i64, ServerError> {
    sqlx::query_scalar::<_, i64>("SELECT revision FROM projects WHERE project_id = $1 FOR UPDATE")
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("project", project_id))
}

pub(super) async fn load_project_update_state(
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

pub(super) async fn update_project(
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

pub(super) async fn delete_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<(), ServerError> {
    sqlx::query("DELETE FROM projects WHERE project_id = $1")
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn user_ref_from_row(row: &sqlx::postgres::PgRow) -> Result<UserRef, ServerError> {
    Ok(UserRef {
        user_id: row.try_get("user_id")?,
        email: row.try_get("email")?,
        display_name: row.try_get("display_name")?,
        avatar_url: row.try_get("avatar_url")?,
        role: row.try_get("role")?,
    })
}

fn admin_org_from_row(row: &sqlx::postgres::PgRow) -> Result<AdminOrg, ServerError> {
    Ok(AdminOrg {
        org_id: row.try_get("org_id")?,
        name: row.try_get("name")?,
        allowed_email_domains: row.try_get("allowed_email_domains")?,
        revision: row.try_get("revision")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn member_from_row(row: &sqlx::postgres::PgRow) -> Result<Member, ServerError> {
    Ok(Member {
        user_id: row.try_get("user_id")?,
        email: row.try_get("email")?,
        display_name: row.try_get("display_name")?,
        role: org_role(row.try_get::<String, _>("role")?.as_str())?,
        status: member_status(row.try_get::<String, _>("status")?.as_str())?,
        external_identity_bound: row.try_get("external_identity_bound")?,
        revision: row.try_get("revision")?,
    })
}

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

fn access_token_meta_from_row(row: &sqlx::postgres::PgRow) -> Result<AccessTokenMeta, ServerError> {
    Ok(AccessTokenMeta {
        token_id: row.try_get("token_id")?,
        user_id: row.try_get("user_id")?,
        kind: access_token_kind(row.try_get::<String, _>("kind")?.as_str())?,
        revoked: row
            .try_get::<Option<OffsetDateTime>, _>("revoked_at")?
            .is_some(),
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
    })
}

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

fn org_role(value: &str) -> Result<OrgRole, ServerError> {
    match value {
        "owner" => Ok(OrgRole::Owner),
        "admin" => Ok(OrgRole::Admin),
        "member" => Ok(OrgRole::Member),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown organization role: {other}"
        ))),
    }
}

fn project_role(value: &str) -> Result<ProjectRole, ServerError> {
    match value {
        "admin" => Ok(ProjectRole::Admin),
        "member" => Ok(ProjectRole::Member),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown project role: {other}"
        ))),
    }
}

fn member_status(value: &str) -> Result<MemberStatus, ServerError> {
    match value {
        "invited" => Ok(MemberStatus::Invited),
        "active" => Ok(MemberStatus::Active),
        "disabled" => Ok(MemberStatus::Disabled),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown member status: {other}"
        ))),
    }
}

fn access_token_kind(value: &str) -> Result<AccessTokenKind, ServerError> {
    match value {
        "access" => Ok(AccessTokenKind::Access),
        "refresh" => Ok(AccessTokenKind::Refresh),
        "integration" => Ok(AccessTokenKind::Integration),
        "web_session" => Ok(AccessTokenKind::WebSession),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown access token kind: {other}"
        ))),
    }
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}
