use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::api::PageInfo;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserRef {
    pub user_id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrgRef {
    pub org_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRef {
    pub project_id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrgRole {
    Owner,
    Admin,
    Member,
}

impl OrgRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    Invited,
    Active,
    Disabled,
}

impl MemberStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invited => "invited",
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminOrg {
    pub org_id: String,
    pub name: String,
    pub allowed_email_domains: Vec<String>,
    pub revision: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateAdminOrgRequest {
    pub name: Option<String>,
    pub allowed_email_domains: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateMemberRequest {
    pub email: String,
    pub role: OrgRole,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateMemberRequest {
    pub role: Option<OrgRole>,
    pub status: Option<MemberStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub user_id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub role: OrgRole,
    pub status: MemberStatus,
    pub external_identity_bound: bool,
    pub revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberListResponse {
    pub items: Vec<Member>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminProject {
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub member_count: i64,
    pub revision: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminProjectListResponse {
    pub items: Vec<AdminProject>,
    pub page_info: PageInfo,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRole {
    Member,
    Admin,
}

impl ProjectRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Admin => "admin",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMember {
    pub project_id: String,
    pub user: UserRef,
    pub role: ProjectRole,
    #[serde(with = "time::serde::rfc3339")]
    pub joined_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemberListResponse {
    pub items: Vec<ProjectMember>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateProjectMemberRequest {
    pub user_id: String,
    pub role: ProjectRole,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateProjectMemberRequest {
    pub role: ProjectRole,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessTokenKind {
    Access,
    Refresh,
    Integration,
    WebSession,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessTokenMeta {
    pub token_id: String,
    pub user_id: String,
    pub kind: AccessTokenKind,
    pub revoked: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessTokenListResponse {
    pub items: Vec<AccessTokenMeta>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEvent {
    pub event_id: String,
    pub actor_user_id: Option<String>,
    pub actor_display_name: Option<String>,
    pub actor_email: Option<String>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub target_display_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEventListResponse {
    pub items: Vec<AuditEvent>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateProjectRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateProjectRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectListResponse {
    pub items: Vec<Project>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub revision: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteResult {
    pub deleted: bool,
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_timestamps_use_rfc3339() {
        let event = AuditEvent {
            event_id: "evt_test".to_owned(),
            actor_user_id: None,
            actor_display_name: None,
            actor_email: None,
            action: "test.created".to_owned(),
            target_type: "test".to_owned(),
            target_display_name: None,
            target_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
        };

        let json = serde_json::to_value(&event).expect("serialize audit event");
        assert_eq!(json["created_at"], "1970-01-01T00:00:00Z");

        let decoded: AuditEvent = serde_json::from_value(json).expect("deserialize audit event");
        assert_eq!(decoded.created_at, OffsetDateTime::UNIX_EPOCH);
    }
}
