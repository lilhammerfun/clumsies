//! Application operations and transaction coordination for organization resources.

use super::repository;
use crate::app::audit_event;
use crate::app::auth::AuthPrincipal;
use crate::app::organization::dto::{
    AdminOrg, CreateMemberRequest, Member, MemberListResponse, MemberStatus, OrgRole,
    UpdateAdminOrgRequest, UpdateMemberRequest,
};
use crate::dto::DeleteResult;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use crate::pagination::admin_page;
use std::collections::BTreeSet;

/// Return organization settings after verifying organization-administrator privileges.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_admin_org(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
) -> Result<AdminOrg, ServerError> {
    let org_id = &principal.org_id;
    principal.require_org_admin()?;

    repository::load_admin_org(pool, org_id).await
}

/// Apply a revision-checked organization settings change and its audit record atomically.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
pub async fn update_admin_org(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    expected_revision: i64,
    request: UpdateAdminOrgRequest,
) -> Result<AdminOrg, ServerError> {
    principal.require_org_admin()?;

    let mut tx = pool.begin().await?;
    let current = repository::lock_admin_org(&mut tx, &principal.org_id).await?;
    if current.revision != expected_revision {
        return Err(ServerError::version_conflict(
            "org",
            expected_revision,
            current.revision,
        ));
    }
    let name = match request.name {
        Some(name) if !name.trim().is_empty() => name.trim().to_owned(),
        Some(_) => {
            return Err(ServerError::InvalidRequest(
                "organization name must not be empty".to_owned(),
            ));
        }
        None => current.name,
    };
    let allowed_email_domains = match request.allowed_email_domains {
        Some(domains) => normalize_email_domains(domains)?,
        None => current.allowed_email_domains,
    };
    repository::update_admin_org(&mut tx, &principal.org_id, &name, &allowed_email_domains).await?;
    audit_event::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.org_updated",
        "org",
        Some(&principal.org_id),
    )
    .await?;
    tx.commit().await?;
    get_admin_org(pool, principal).await
}

/// Return the administrator-visible member page with filtering applied before pagination.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_admin_members(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    offset: i64,
    limit: i64,
    query: Option<&str>,
) -> Result<MemberListResponse, ServerError> {
    principal.require_org_admin()?;

    let query = query.map(str::trim).filter(|query| !query.is_empty());
    let items = repository::list_admin_members(pool, offset, limit + 1, query).await?;
    let (items, page_info) = admin_page(items, offset, limit);
    Ok(MemberListResponse { items, page_info })
}

/// Validate an invitation's role and email policy, then persist the member and audit event.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
pub async fn create_admin_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    request: CreateMemberRequest,
) -> Result<Member, ServerError> {
    principal.require_org_admin()?;

    if request.role == OrgRole::Owner && principal.role != "owner" {
        return Err(ServerError::Forbidden(
            "only an organization owner can create another owner".to_owned(),
        ));
    }
    let email = normalize_email(&request.email)?;
    let allowed_domains = repository::load_allowed_email_domains(pool, &principal.org_id).await?;
    enforce_invited_email_domain(&email, &allowed_domains)?;
    if repository::member_email_exists(pool, &email).await? {
        return Err(ServerError::InvalidRequest(
            "a member with this email already exists".to_owned(),
        ));
    }
    let user_id = prefixed_id("usr");
    let mut tx = pool.begin().await?;
    repository::insert_member(&mut tx, &user_id, &email, request.role.as_str()).await?;
    audit_event::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.member_created",
        "user",
        Some(&user_id),
    )
    .await?;
    tx.commit().await?;
    repository::load_member(pool, &user_id).await
}

/// Apply a member change while protecting the caller and the last active owner.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
pub async fn update_admin_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    user_id: &str,
    expected_revision: i64,
    request: UpdateMemberRequest,
) -> Result<Member, ServerError> {
    principal.require_org_admin()?;

    if user_id == principal.user_id
        && (request
            .role
            .is_some_and(|role| role.as_str() != principal.role)
            || request
                .status
                .is_some_and(|status| status != MemberStatus::Active))
    {
        return Err(ServerError::InvalidRequest(
            "the current user cannot change their own role or active status".to_owned(),
        ));
    }
    let mut tx = pool.begin().await?;
    let current = repository::lock_member(&mut tx, user_id).await?;
    if current.revision != expected_revision {
        return Err(ServerError::version_conflict(
            "member",
            expected_revision,
            current.revision,
        ));
    }
    let next_role = request
        .role
        .map(|role| role.as_str().to_owned())
        .unwrap_or_else(|| current.role.clone());
    let next_status = request
        .status
        .map(|status| status.as_str().to_owned())
        .unwrap_or_else(|| current.status.clone());
    if principal.role != "owner" && (current.role == "owner" || next_role == "owner") {
        return Err(ServerError::Forbidden(
            "only an organization owner can modify an owner account".to_owned(),
        ));
    }
    let removes_active_owner = current.role == "owner"
        && current.status == "active"
        && (next_role != "owner" || next_status != "active");
    if removes_active_owner && repository::active_owner_count(&mut tx).await? <= 1 {
        return Err(ServerError::InvalidRequest(
            "the last active organization owner cannot be disabled or demoted".to_owned(),
        ));
    }
    repository::update_member(&mut tx, user_id, &next_role, &next_status).await?;
    crate::app::inbox::notify_access_change(
        &mut tx,
        principal,
        user_id,
        None,
        Some(&current.role),
        Some(&next_role),
    )
    .await?;
    if next_status == "disabled" {
        repository::revoke_user_sessions(&mut tx, &principal.org_id, user_id).await?;
    }
    audit_event::insert_audit_event(
        &mut tx,
        &principal.org_id,
        Some(&principal.user_id),
        "admin.member_updated",
        "user",
        Some(user_id),
    )
    .await?;
    tx.commit().await?;
    repository::load_member(pool, user_id).await
}

/// Disable a member and revoke its credentials while preserving an active organization owner.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
pub async fn delete_admin_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    user_id: &str,
    expected_revision: i64,
) -> Result<DeleteResult, ServerError> {
    principal.require_org_admin()?;

    if user_id == principal.user_id {
        return Err(ServerError::InvalidRequest(
            "the current user cannot disable their own account".to_owned(),
        ));
    }
    update_admin_member(
        pool,
        principal,
        user_id,
        expected_revision,
        UpdateMemberRequest {
            role: None,
            status: Some(MemberStatus::Disabled),
        },
    )
    .await?;
    Ok(DeleteResult {
        deleted: true,
        id: user_id.to_owned(),
    })
}

/// Normalize an email address and reject malformed identity input.
///
/// # Errors
/// Returns invalid input when the email has no valid local part or domain.
fn normalize_email(email: &str) -> Result<String, ServerError> {
    let email = email.trim().to_ascii_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(ServerError::InvalidRequest(
            "invalid member email".to_owned(),
        ));
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') || !valid_domain(domain) {
        return Err(ServerError::InvalidRequest(
            "invalid member email".to_owned(),
        ));
    }
    Ok(email)
}

/// Normalize and deduplicate allowed domains while rejecting invalid labels.
///
/// # Errors
/// Returns invalid input for malformed, empty, or unsupported domain entries.
fn normalize_email_domains(domains: Vec<String>) -> Result<Vec<String>, ServerError> {
    let mut normalized = BTreeSet::new();
    for domain in domains {
        let domain = domain.trim().trim_start_matches('@').to_ascii_lowercase();
        if !valid_domain(&domain) {
            return Err(ServerError::InvalidRequest(format!(
                "invalid allowed email domain: {domain}"
            )));
        }
        normalized.insert(domain);
    }
    Ok(normalized.into_iter().collect())
}

/// Check DNS-style email-domain labels without performing network resolution.
fn valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// Require an invited email to satisfy the organization's configured domain allowlist.
///
/// # Errors
/// Rejects an invitation outside the configured domain allowlist.
fn enforce_invited_email_domain(
    email: &str,
    allowed_domains: &[String],
) -> Result<(), ServerError> {
    if allowed_domains.is_empty() {
        return Ok(());
    }
    let domain = email
        .rsplit_once('@')
        .map(|(_, domain)| domain)
        .unwrap_or("");
    if allowed_domains.iter().any(|allowed| allowed == domain) {
        Ok(())
    } else {
        Err(ServerError::InvalidRequest(
            "member email is outside the organization allowlist".to_owned(),
        ))
    }
}

// Transaction participants share the caller-owned transaction; only the outer service commits.
