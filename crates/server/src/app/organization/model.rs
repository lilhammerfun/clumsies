//! Internal values and invariants for organization resources.

use crate::app::organization::dto::{MemberStatus, OrgRole};
use crate::error::ServerError;

pub(crate) struct LockedAdminOrg {
    pub(crate) name: String,
    pub(crate) allowed_email_domains: Vec<String>,
    pub(crate) revision: i64,
}

pub(crate) struct LockedMember {
    pub(crate) role: String,
    pub(crate) status: String,
    pub(crate) revision: i64,
}

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
