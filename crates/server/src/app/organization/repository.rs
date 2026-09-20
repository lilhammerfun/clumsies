//! SQL queries and persistence for organization resources.

use super::model::{LockedAdminOrg, LockedMember, member_status, org_role};
use crate::app::organization::dto::{AdminOrg, Member, OrgRef, UserRef};
use crate::error::ServerError;
use sqlx::{FromRow, PgPool, Postgres, Row, Transaction};

/// Read public user identity without credential material.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_user_ref(
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
    UserRef::from_row(&row).map_err(ServerError::from)
}

/// Read the organization's public identity for embedding in other responses.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_org_ref(
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

/// Read administrative organization settings and their concurrency revision.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_admin_org(pool: &PgPool, org_id: &str) -> Result<AdminOrg, ServerError> {
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

/// Lock organization settings before a revision-checked mutation.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn lock_admin_org(
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

/// Persist normalized organization settings at the next concurrency revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn update_admin_org(
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

/// Read organization members with search and status filters applied before pagination.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn list_admin_members(
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

/// Read the organization's admission allowlist before inviting a member.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_allowed_email_domains(
    pool: &PgPool,
    org_id: &str,
) -> Result<Vec<String>, ServerError> {
    sqlx::query_scalar::<_, Vec<String>>("SELECT allowed_email_domains FROM orgs WHERE org_id = $1")
        .bind(org_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ServerError::not_found("org", org_id))
}

/// Check whether the normalized member email already exists.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn member_email_exists(pool: &PgPool, email: &str) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM users WHERE lower(email) = lower($1))",
    )
    .bind(email)
    .fetch_one(pool)
    .await?)
}

/// Persist a normalized invited member and its initial concurrency revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn insert_member(
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

/// Lock a member's current role, status, and revision before administration.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn lock_member(
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

/// Count enabled owners while enforcing the last-owner invariant.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn active_owner_count(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<i64, ServerError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM users WHERE role = 'owner' AND status = 'active'",
    )
    .fetch_one(&mut **tx)
    .await?)
}

/// Persist the role and enabled state selected by the organization operation.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn update_member(
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

/// Invalidate a user's sessions and credentials as part of membership administration.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn revoke_user_sessions(
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

/// Read one required member's public identity and administrative state.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_member(pool: &PgPool, user_id: &str) -> Result<Member, ServerError> {
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

/// Read an organization's member state before granting project membership.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_user_status(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
) -> Result<String, ServerError> {
    sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("user", user_id))
}

/// Decode organization settings from a persisted row.
///
/// # Errors
/// Propagates database access and row-decoding failures.
fn admin_org_from_row(row: &sqlx::postgres::PgRow) -> Result<AdminOrg, ServerError> {
    Ok(AdminOrg {
        org_id: row.try_get("org_id")?,
        name: row.try_get("name")?,
        allowed_email_domains: row.try_get("allowed_email_domains")?,
        revision: row.try_get("revision")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Decode public identity and administrative membership state from a joined row.
///
/// # Errors
/// Propagates database access and row-decoding failures.
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
