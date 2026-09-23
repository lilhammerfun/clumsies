//! Internal values and invariants for project resources.

use crate::app::project::dto::ProjectRole;
use crate::error::ServerError;

/// Project metadata and revision read under an update lock.
pub(crate) struct LockedProject {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub(crate) name: String,
    /// Human-readable explanation associated with the resource.
    pub(crate) description: String,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub(crate) revision: i64,
}

/// Persisted idempotency claim tying a caller's creation key to one project payload.
pub(crate) struct ProjectCreation {
    /// Project boundary containing the resource or proposal.
    pub(crate) project_id: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub(crate) name: String,
    /// Human-readable explanation associated with the resource.
    pub(crate) description: String,
}

/// Project ownership and metadata loaded before a revision-checked mutation.
pub(crate) struct ProjectUpdateState {
    /// Organization boundary to which the resource or identity belongs.
    pub(crate) org_id: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub(crate) name: String,
    /// Human-readable explanation associated with the resource.
    pub(crate) description: String,
}

/// Decode project-local privileges, rejecting unknown persisted values.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
pub(crate) fn project_role(value: &str) -> Result<ProjectRole, ServerError> {
    match value {
        "owner" => Ok(ProjectRole::Owner),
        "admin" => Ok(ProjectRole::Admin),
        "member" => Ok(ProjectRole::Member),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown project role: {other}"
        ))),
    }
}
