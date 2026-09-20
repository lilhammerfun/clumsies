//! Request and response data for project resources.

use crate::app::organization::dto::UserRef;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Compact accessible-project identity and the user's project role.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRef {
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Project-local privileges; they do not grant organization administration.
    pub role: ProjectRole,
}

/// Eligible organization members with continuation metadata for project administration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemberCandidateListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<UserRef>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Administrative project view including membership count and concurrency revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminProject {
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Human-readable explanation associated with the resource.
    pub description: String,
    /// Number of users currently assigned to the project.
    pub member_count: i64,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub revision: i64,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// UTC timestamp of the latest persisted change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Administrative project page with continuation metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminProjectListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<AdminProject>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Project-local privileges independent of organization administration.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRole {
    /// Project participation without project-administration privileges.
    Member,
    /// Project-local administration granted independently of organization role.
    Admin,
}

impl ProjectRole {
    /// Return the stable string representation used by the surrounding persistence or protocol
    /// contract.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Admin => "admin",
        }
    }
}

/// User identity and project-specific privileges within a membership record.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMember {
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Public identity of the authenticated user.
    pub user: UserRef,
    /// Project-local privileges; they do not grant organization administration.
    pub role: ProjectRole,
    /// UTC time at which the user joined the project.
    #[serde(with = "time::serde::rfc3339")]
    pub joined_at: OffsetDateTime,
}

/// Project membership page with continuation metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemberListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<ProjectMember>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Organization identity and project role to add to a project's membership.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateProjectMemberRequest {
    /// Stable identity of the user represented or targeted by this record.
    pub user_id: String,
    /// Project-local privileges; they do not grant organization administration.
    pub role: ProjectRole,
}

/// Replacement project-specific role for an existing member.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateProjectMemberRequest {
    /// Project-local privileges; they do not grant organization administration.
    pub role: ProjectRole,
}

/// Name and optional description of a project to create.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateProjectRequest {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Human-readable explanation associated with the resource.
    pub description: Option<String>,
}

/// Optional project name and description edits at an expected revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateProjectRequest {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: Option<String>,
    /// Human-readable explanation associated with the resource.
    pub description: Option<String>,
}

/// Accessible project collection with pagination metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<Project>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Public project metadata and optimistic concurrency version.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Human-readable explanation associated with the resource.
    pub description: String,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub revision: i64,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// UTC timestamp of the latest persisted change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Role filter and page input for project membership administration.
#[derive(Debug, Deserialize)]
pub(crate) struct ListAdminProjectMembersQuery {
    /// Project-local privileges; they do not grant organization administration.
    pub(super) role: Option<String>,
    /// Maximum results requested for this page, subject to API bounds.
    pub(super) limit: Option<String>,
    /// Opaque server position used to resume synchronization or pagination.
    pub(super) cursor: Option<String>,
}
