//! Request and response data for review resources.

use crate::app::draft::dto::{
    Draft, DraftCoordination, DraftOperation, ReconciliationResourceState,
};
use crate::app::organization::dto::UserRef;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewDraftRequest {
    pub draft_id: String,
    pub expected_draft_version: i64,
    pub candidate_id: Option<String>,
    pub resolved_state: Option<ReconciliationResourceState>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewRequest {
    pub drafts: Vec<ReviewDraftRequest>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewSubmissionRequest {
    pub expected_review_version: i64,
    pub drafts: Vec<ReviewDraftRequest>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Open,
    Approved,
    Rejected,
    Merged,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Review {
    pub review_id: String,
    pub project_id: String,
    pub draft_id: String,
    pub draft_ids: Vec<String>,
    pub author: UserRef,
    pub title: String,
    pub description: String,
    pub status: ReviewStatus,
    pub version: i64,
    pub decision_body: Option<String>,
    pub approved_result_hash: Option<String>,
    pub decided_by: Option<UserRef>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub coordination: DraftCoordination,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewDetail {
    pub review: Review,
    pub draft: Draft,
    pub operations: Vec<DraftOperation>,
    pub drafts: Vec<ReviewDraftDetail>,
    pub comments: Vec<ReviewComment>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewDraftDetail {
    pub draft: Draft,
    pub operations: Vec<DraftOperation>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewListResponse {
    pub items: Vec<Review>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewCommentListResponse {
    pub items: Vec<ReviewComment>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewComment {
    pub comment_id: String,
    pub review_id: String,
    pub author: UserRef,
    pub body: String,
    #[serde(default)]
    pub anchor_path: Option<String>,
    #[serde(default)]
    pub anchor_line: Option<i64>,
    pub review_version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewCommentRequest {
    pub body: String,
    pub expected_review_version: i64,
    #[serde(default)]
    pub anchor_path: Option<String>,
    #[serde(default)]
    pub anchor_line: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewDecisionRequest {
    pub decision: ReviewDecision,
    pub expected_review_version: i64,
    pub body: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approved,
    Rejected,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewMergeRequest {
    pub expected_review_version: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewMergeResult {
    pub review: Review,
    pub commit_id: Option<String>,
    pub applied_operation_count: i64,
}

#[derive(Deserialize)]
pub(crate) struct ListReviewsQuery {
    pub(super) project_id: Option<String>,
}
