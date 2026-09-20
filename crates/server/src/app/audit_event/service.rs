//! Application operations and transaction coordination for audit event resources.

use super::repository;
use crate::app::audit_event::dto::AuditEventListResponse;
use crate::app::auth::AuthPrincipal;
use crate::error::ServerError;
use crate::pagination::admin_page;

/// Return administrator-visible audit history with search applied before pagination.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_admin_audit_events(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    offset: i64,
    limit: i64,
    query: Option<&str>,
) -> Result<AuditEventListResponse, ServerError> {
    let org_id = &principal.org_id;
    principal.require_org_admin()?;

    let query = query.map(str::trim).filter(|query| !query.is_empty());
    let items = repository::list_audit_events(pool, org_id, offset, limit + 1, query).await?;
    let (items, page_info) = admin_page(items, offset, limit);
    Ok(AuditEventListResponse { items, page_info })
}

// Transaction participants share the caller-owned transaction; only the outer service commits.
