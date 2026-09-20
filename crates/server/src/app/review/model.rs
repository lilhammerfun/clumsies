//! Internal values and invariants for review resources.

use crate::app::review::dto::ReviewStatus;
use crate::error::ServerError;

/// Retain automatic merges around conflicting sections using collision-free diff3 markers.
pub(super) fn conflict_content(
    candidate: &crate::app::draft::dto::DraftReconciliationCandidate,
) -> Option<super::dto::ReviewConflictContent> {
    let conflict = candidate.conflicts.iter().find(|c| c.field == "content")?;
    let base = conflict.base.as_deref().unwrap_or_default();
    let shared = conflict.current.as_deref().unwrap_or_default();
    let proposed = conflict.draft.as_deref().unwrap_or_default();
    let marker_length = [base, shared, proposed]
        .iter()
        .flat_map(|text| text.lines())
        .map(|line| {
            line.chars()
                .take_while(|c| ['<', '>', '|', '='].contains(c))
                .count()
                + 1
        })
        .max()
        .unwrap_or(7)
        .max(7);
    let text = diffy::MergeOptions::new()
        .set_conflict_marker_length(marker_length)
        .merge(base, shared, proposed)
        .err()?;
    Some(super::dto::ReviewConflictContent {
        text,
        marker_length,
    })
}

/// Count final-content lines using the same trailing-newline convention as client anchors.
pub(crate) fn review_comment_line_count(content: &str) -> i64 {
    if content.is_empty() {
        0
    } else {
        content.split('\n').count() as i64
    }
}

/// Decode the persisted review lifecycle before applying publication rules.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
pub(crate) fn review_status(value: &str) -> Result<ReviewStatus, ServerError> {
    match value {
        "open" => Ok(ReviewStatus::Open),
        "approved" => Ok(ReviewStatus::Approved),
        "rejected" => Ok(ReviewStatus::Rejected),
        "merged" => Ok(ReviewStatus::Merged),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown review status: {other}"
        ))),
    }
}

/// Committed snapshot identity and operation count returned by transactional publication.
pub(crate) struct ReviewMergeData {
    /// Stable identifier of the immutable committed snapshot.
    pub(crate) commit_id: String,
    /// Number of resource operations materialized by this publication.
    pub(crate) applied_operation_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_comment_line_count_matches_client_line_numbering() {
        assert_eq!(review_comment_line_count(""), 0);
        assert_eq!(review_comment_line_count("one"), 1);
        assert_eq!(review_comment_line_count("one\ntwo"), 2);
        assert_eq!(review_comment_line_count("one\n"), 2);
        assert_eq!(review_comment_line_count("one\n\n"), 3);
    }
}
