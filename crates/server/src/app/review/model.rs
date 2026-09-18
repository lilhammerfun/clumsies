//! Internal values and invariants for review resources.

use crate::app::review::dto::ReviewStatus;
use crate::error::ServerError;

pub(crate) fn review_comment_line_count(content: &str) -> i64 {
    if content.is_empty() {
        0
    } else {
        content.split('\n').count() as i64
    }
}

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

pub(crate) struct ReviewMergeData {
    pub(crate) commit_id: String,
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
