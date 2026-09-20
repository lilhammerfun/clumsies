//! Request and response data for organization resources.

use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Public identity fields safe to embed in resource responses.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct UserRef {
    /// Stable identity of the user represented or targeted by this record.
    pub user_id: String,
    /// User email used for organization admission and identity display.
    pub email: String,
    /// Optional human-readable name supplied by the identity provider or administrator.
    pub display_name: Option<String>,
    /// Optional image URL supplied by the verified identity provider.
    pub avatar_url: Option<String>,
    /// Organization-wide privileges; project membership is checked separately.
    pub role: String,
}

/// Public organization identity used in account and project responses.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrgRef {
    /// Organization boundary to which the resource or identity belongs.
    pub org_id: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
}

/// Organization-wide privileges granted to an authenticated member.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrgRole {
    /// Organization administration with the protected last-active-owner invariant.
    Owner,
    /// Organization-wide administration without the owner's protected lifecycle role.
    Admin,
    /// Ordinary organization membership; project access requires explicit project membership.
    Member,
}

impl OrgRole {
    /// Return the stable string representation used by the surrounding persistence or protocol
    /// contract.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// Whether an invited or established organization member may authenticate.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    /// The email is admitted but has not yet completed external identity binding.
    Invited,
    /// The established member may authenticate while credentials remain valid.
    Active,
    /// The member is denied authentication and its existing sessions are revoked.
    Disabled,
}

impl MemberStatus {
    /// Return the stable string representation used by the surrounding persistence or protocol
    /// contract.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invited => "invited",
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

/// Administrative organization settings and their concurrency revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminOrg {
    /// Organization boundary to which the resource or identity belongs.
    pub org_id: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Permitted email domains; an empty collection imposes no domain restriction.
    pub allowed_email_domains: Vec<String>,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub revision: i64,
    /// UTC timestamp of the latest persisted change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Optional organization settings to replace at an expected revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateAdminOrgRequest {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: Option<String>,
    /// Permitted email domains; an empty collection imposes no domain restriction.
    pub allowed_email_domains: Option<Vec<String>>,
}

/// Email and organization role to grant to an invited member.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateMemberRequest {
    /// User email used for organization admission and identity display.
    pub email: String,
    /// Organization-wide privileges; project membership is checked separately.
    pub role: OrgRole,
}

/// Optional organization role and enabled-state changes for a member.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateMemberRequest {
    /// Organization-wide privileges; project membership is checked separately.
    pub role: Option<OrgRole>,
    /// Requested membership state; omission preserves the current state.
    pub status: Option<MemberStatus>,
}

/// Organization membership, identity-binding state, and concurrency revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    /// Stable identity of the user represented or targeted by this record.
    pub user_id: String,
    /// User email used for organization admission and identity display.
    pub email: String,
    /// Optional human-readable name supplied by the identity provider or administrator.
    pub display_name: Option<String>,
    /// Organization-wide privileges; project membership is checked separately.
    pub role: OrgRole,
    /// Admission and enabled state of this organization member.
    pub status: MemberStatus,
    /// Whether this member has completed binding to an external identity.
    pub external_identity_bound: bool,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub revision: i64,
}

/// Organization membership page with continuation metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<Member>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}
