//! Errors shared by resource operations.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("{entity} not found: {id}")]
    NotFound { entity: &'static str, id: String },
    #[error("{entity} already exists: {id}")]
    AlreadyExists { entity: &'static str, id: String },
    #[error("{entity} version conflict: expected {expected}, actual {actual}")]
    VersionConflict {
        entity: &'static str,
        expected: i64,
        actual: i64,
    },
    #[error("ref precondition failed: expected {expected:?}, actual {actual:?}")]
    PreconditionFailed {
        expected: Option<String>,
        actual: Option<String>,
    },
    #[error("draft {draft_id} requires reconciliation candidate {candidate_id}")]
    ReconciliationRequired {
        draft_id: String,
        candidate_id: String,
        current_commit_id: Option<String>,
    },
    #[error("draft {draft_id} is already based on the current ref")]
    DraftAlreadyCurrent { draft_id: String },
    #[error("reconciliation candidate is no longer valid: {candidate_id}")]
    ReconciliationCandidateInvalid { candidate_id: String },
    #[error("{entity} cannot transition from {from} to {to}")]
    InvalidTransition {
        entity: &'static str,
        from: String,
        to: String,
    },
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl ServerError {
    pub(crate) fn not_found(entity: &'static str, id: impl Into<String>) -> Self {
        Self::NotFound {
            entity,
            id: id.into(),
        }
    }

    pub(crate) fn already_exists(entity: &'static str, id: impl Into<String>) -> Self {
        Self::AlreadyExists {
            entity,
            id: id.into(),
        }
    }

    pub(crate) fn version_conflict(entity: &'static str, expected: i64, actual: i64) -> Self {
        Self::VersionConflict {
            entity,
            expected,
            actual,
        }
    }

    pub(crate) fn invalid_transition(entity: &'static str, from: &str, to: &str) -> Self {
        Self::InvalidTransition {
            entity,
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    pub(crate) fn precondition_failed(expected: Option<&str>, actual: Option<&str>) -> Self {
        Self::PreconditionFailed {
            expected: expected.map(ToOwned::to_owned),
            actual: actual.map(ToOwned::to_owned),
        }
    }
}
