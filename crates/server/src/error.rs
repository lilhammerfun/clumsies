//! Errors shared by resource operations.

use thiserror::Error;

/// Shared resource-operation failures retaining database diagnostics for internal handling.
#[derive(Debug, Error)]
pub enum ServerError {
    /// The authenticated actor lacks permission for the requested operation.
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// The requested resource is absent or hidden by its authorization boundary.
    #[error("{entity} not found: {id}")]
    NotFound {
        /// Resource category whose lookup or invariant failed.
        entity: &'static str,
        /// Stable identifier of the resource described by this result.
        id: String,
    },
    /// A resource identity or unique name is already in use.
    #[error("{entity} already exists: {id}")]
    AlreadyExists {
        /// Resource category whose lookup or invariant failed.
        entity: &'static str,
        /// Stable identifier of the resource described by this result.
        id: String,
    },
    /// The client's expected resource revision differs from the stored revision.
    #[error("{entity} version conflict: expected {expected}, actual {actual}")]
    VersionConflict {
        /// Resource category whose lookup or invariant failed.
        entity: &'static str,
        /// Caller-supplied value required for the operation to proceed.
        expected: i64,
        /// Observed value that caused the precondition to fail.
        actual: i64,
    },
    /// The supplied snapshot validator does not match the current reference head.
    #[error("ref precondition failed: expected {expected:?}, actual {actual:?}")]
    PreconditionFailed {
        /// Caller-supplied value required for the operation to proceed.
        expected: Option<String>,
        /// Observed value that caused the precondition to fail.
        actual: Option<String>,
    },
    /// Upstream changes require the client to inspect the persisted candidate before continuing.
    #[error("draft {draft_id} requires reconciliation candidate {candidate_id}")]
    ReconciliationRequired {
        /// Stable identifier of the editable proposal.
        draft_id: String,
        /// Identifier of the reconciliation result being inspected or applied.
        candidate_id: String,
        /// Reference head observed when computing freshness or reconciliation.
        current_commit_id: Option<String>,
    },
    /// Rebase was requested for a proposal already based on the current head.
    #[error("draft {draft_id} is already based on the current ref")]
    DraftAlreadyCurrent {
        /// Stable identifier of the editable proposal.
        draft_id: String,
    },
    /// Stored evidence no longer matches the proposal, upstream reference, or applicable
    /// lifecycle.
    #[error("reconciliation candidate is no longer valid: {candidate_id}")]
    ReconciliationCandidateInvalid {
        /// Identifier of the reconciliation result being inspected or applied.
        candidate_id: String,
    },
    /// The requested lifecycle change is not allowed from the current resource state.
    #[error("{entity} cannot transition from {from} to {to}")]
    InvalidTransition {
        /// Resource category whose lookup or invariant failed.
        entity: &'static str,
        /// Original lifecycle state rejected by the requested transition.
        from: String,
        /// Requested lifecycle state that could not be entered.
        to: String,
    },
    /// Input violates a resource invariant or contains an unsupported persisted value.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Database access or row decoding failed; technical details stay internal to HTTP handling.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl ServerError {
    /// Construct a resource lookup failure with its safe public identity.
    pub(crate) fn not_found(entity: &'static str, id: impl Into<String>) -> Self {
        Self::NotFound {
            entity,
            id: id.into(),
        }
    }

    /// Construct a duplicate-resource failure with its conflicting public identity.
    pub(crate) fn already_exists(entity: &'static str, id: impl Into<String>) -> Self {
        Self::AlreadyExists {
            entity,
            id: id.into(),
        }
    }

    /// Report the current stored revision when the client's expected revision is stale.
    pub(crate) fn version_conflict(entity: &'static str, expected: i64, actual: i64) -> Self {
        Self::VersionConflict {
            entity,
            expected,
            actual,
        }
    }

    /// Report a forbidden lifecycle transition and its resource identity.
    pub(crate) fn invalid_transition(entity: &'static str, from: &str, to: &str) -> Self {
        Self::InvalidTransition {
            entity,
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    /// Report a failed optimistic precondition with the current HTTP validator.
    pub(crate) fn precondition_failed(expected: Option<&str>, actual: Option<&str>) -> Self {
        Self::PreconditionFailed {
            expected: expected.map(ToOwned::to_owned),
            actual: actual.map(ToOwned::to_owned),
        }
    }
}
