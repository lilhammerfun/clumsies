//! Internal values and invariants for organization resources.

use crate::app::organization::dto::{MemberStatus, OrgRole};
use crate::error::ServerError;

/// Organization settings read under a lock before a revision-checked update.
pub(crate) struct LockedAdminOrg {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub(crate) name: String,
    /// Permitted email domains; an empty collection imposes no domain restriction.
    pub(crate) allowed_email_domains: Vec<String>,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub(crate) revision: i64,
}

/// Membership privileges and revision read before an administrative mutation.
pub(crate) struct LockedMember {
    /// Organization-wide privileges; project membership is checked separately.
    pub(crate) role: String,
    /// Current membership state read under the administrative lock.
    pub(crate) status: String,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub(crate) revision: i64,
}

/// Decode an organization privilege level, rejecting unknown persisted values.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
pub(crate) fn org_role(value: &str) -> Result<OrgRole, ServerError> {
    match value {
        "owner" => Ok(OrgRole::Owner),
        "admin" => Ok(OrgRole::Admin),
        "member" => Ok(OrgRole::Member),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown organization role: {other}"
        ))),
    }
}

/// Decode the persisted organization membership lifecycle.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
pub(crate) fn member_status(value: &str) -> Result<MemberStatus, ServerError> {
    match value {
        "invited" => Ok(MemberStatus::Invited),
        "active" => Ok(MemberStatus::Active),
        "disabled" => Ok(MemberStatus::Disabled),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown member status: {other}"
        ))),
    }
}
