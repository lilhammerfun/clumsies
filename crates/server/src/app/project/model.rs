//! Internal values and invariants for project resources.

use crate::app::project::dto::ProjectRole;
use crate::error::ServerError;

pub(crate) struct LockedProject {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) revision: i64,
}

pub(crate) struct ProjectCreation {
    pub(crate) project_id: String,
    pub(crate) name: String,
    pub(crate) description: String,
}

pub(crate) struct ProjectUpdateState {
    pub(crate) org_id: String,
    pub(crate) name: String,
    pub(crate) description: String,
}

pub(crate) fn project_role(value: &str) -> Result<ProjectRole, ServerError> {
    match value {
        "admin" => Ok(ProjectRole::Admin),
        "member" => Ok(ProjectRole::Member),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown project role: {other}"
        ))),
    }
}
