//! Request and response data for review resources.

use crate::app::draft::dto::{
    Draft, DraftCoordination, DraftOperation, DraftReconciliationCandidate,
    ReconciliationResourceState,
};
use crate::app::organization::dto::UserRef;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Expected proposal revision and optional reconciliation resolution for submission.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewDraftRequest {
    /// Stable identifier of the editable proposal.
    pub draft_id: String,
    /// Proposal revision on which the caller's mutation is based.
    pub expected_draft_version: i64,
    /// Identifier of the reconciliation result being inspected or applied.
    pub candidate_id: Option<String>,
    /// User-supplied final state for a conflicting reconciliation candidate.
    pub resolved_state: Option<ReconciliationResourceState>,
}

/// Ordered proposals and optional metadata for a new review.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewRequest {
    /// Selected Project results to propose independently after Project publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_contribution: Option<Vec<OrgContributionEntry>>,
    /// Proposals included in the response, export, or review.
    pub drafts: Vec<ReviewDraftRequest>,
    /// Human-readable summary of a proposal or review.
    pub title: Option<String>,
    /// Human-readable explanation associated with the resource.
    pub description: Option<String>,
}

/// One Project proposal result and its explicit Organization destination.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrgContributionEntry {
    /// Project draft included in this Review whose published result will be copied.
    pub draft_id: String,
    /// Existing Organization identity to update, or absent to create a new resource.
    pub target_id: Option<String>,
    /// Destination for a new Organization resource; defaults to the published Project path.
    pub path: Option<String>,
}

/// Durable contribution intent and the independently reviewed result, if created.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrgContribution {
    /// Selected Project results and explicit destinations.
    pub entries: Vec<OrgContributionEntry>,
    /// Fixed Project publication from which content is copied.
    pub source_commit_id: Option<String>,
    /// Independent Organization Review; retries always return this identity.
    pub org_review_id: Option<String>,
    /// Last creation failure, retained without rolling back Project publication.
    pub last_error: Option<String>,
}

/// Immutable Project publication that originated an Organization contribution.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectReviewSource {
    /// Source Project Review identity.
    pub review_id: String,
    /// Fixed source Project snapshot.
    pub commit_id: String,
}

/// Updated proposal revisions and reconciliation data for a rejected review.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewSubmissionRequest {
    /// Optional replacement contribution choices when resubmitting a Project Review.
    #[serde(default)]
    pub org_contribution: Option<Vec<OrgContributionEntry>>,
    /// Review revision on which the caller's decision or mutation is based.
    pub expected_review_version: i64,
    /// Proposals included in the response, export, or review.
    pub drafts: Vec<ReviewDraftRequest>,
    /// Human-readable summary of a proposal or review.
    pub title: Option<String>,
    /// Human-readable explanation associated with the resource.
    pub description: Option<String>,
}

/// Review revision to inspect before preparing updates for all its proposals.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewUpdatePlanRequest {
    /// Exact review revision currently displayed by the author.
    pub expected_review_version: i64,
}

/// One consistent review snapshot and the updates requiring author confirmation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewUpdatePlan {
    /// Snapshot whose proposal revisions must be submitted together.
    pub detail: ReviewDetail,
    /// Reconciliation results for every behind proposal, in review order.
    pub candidates: Vec<DraftReconciliationCandidate>,
}

/// Author-confirmed updates to the complete ordered proposal set.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewUpdateRequest {
    /// Review revision from the inspected plan.
    pub expected_review_version: i64,
    /// All reviewed proposals, with resolutions for those requiring updates.
    pub drafts: Vec<ReviewDraftRequest>,
}

/// Lifecycle governing review decisions, resubmission, and publication.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// The submitted content is awaiting an administrative decision.
    Open,
    /// The recorded content fingerprint has been accepted for publication.
    Approved,
    /// The submission was rejected and may be revised before resubmission.
    Rejected,
    /// Reviewed content was published and the review is complete.
    Merged,
}

/// Lifecycle and approval state for an ordered set of resource proposals.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Review {
    /// Optional separately reviewed Organization contribution after this Project merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_contribution: Option<OrgContribution>,
    /// Source of this independently reviewed Organization contribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_source: Option<ProjectReviewSource>,
    /// Single authority to which this review publishes.
    pub scope: crate::app::memory::dto::ResourceScope,
    /// Stable identifier of a review spanning one or more proposals.
    pub review_id: String,
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Stable identifier of the editable proposal.
    pub draft_id: String,
    /// Ordered proposals participating in the review or batch.
    pub draft_ids: Vec<String>,
    /// Public identity of the author of this record.
    pub author: UserRef,
    /// Human-readable summary of a proposal or review.
    pub title: String,
    /// Human-readable explanation associated with the resource.
    pub description: String,
    /// Review lifecycle controlling decisions, resubmission, and publication.
    pub status: ReviewStatus,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub version: i64,
    /// Optional explanation attached to the latest review decision.
    pub decision_body: Option<String>,
    /// Content fingerprint covered by the current review approval.
    pub approved_result_hash: Option<String>,
    /// Public identity of the user who recorded the review decision.
    pub decided_by: Option<UserRef>,
    /// UTC time at which the review decision was recorded.
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    /// Freshness and reconciliation information relative to the current reference.
    pub coordination: DraftCoordination,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// UTC timestamp of the latest persisted change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Review metadata, ordered draft details, and discussion required by the review UI.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewDetail {
    /// Review metadata associated with these proposals or publication results.
    pub review: Review,
    /// Proposal metadata and state associated with this response.
    pub draft: Draft,
    /// Ordered mutations applied to the proposal's base state.
    pub operations: Vec<DraftOperation>,
    /// Proposals included in the response, export, or review.
    pub drafts: Vec<ReviewDraftDetail>,
    /// Discussion attached to the review, in repository-defined order.
    pub comments: Vec<ReviewComment>,
}

/// One proposal and its ordered operations within a review.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewDraftDetail {
    /// Proposal metadata and state associated with this response.
    pub draft: Draft,
    /// Ordered mutations applied to the proposal's base state.
    pub operations: Vec<DraftOperation>,
}

/// Accessible review collection with pagination metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<Review>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Chronologically ordered review discussion with pagination metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewCommentListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<ReviewComment>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Discussion text anchored, when requested, to final reviewed content.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewComment {
    /// Stable identifier of a review comment.
    pub comment_id: String,
    /// Stable identifier of a review spanning one or more proposals.
    pub review_id: String,
    /// Public identity of the author of this record.
    pub author: UserRef,
    /// Stored text content, including Markdown where the resource contract permits it.
    pub body: String,
    /// Final reviewed resource path to which the comment belongs.
    #[serde(default)]
    pub anchor_path: Option<String>,
    /// One-based line in the final reviewed content, when the comment is anchored.
    #[serde(default)]
    pub anchor_line: Option<i64>,
    /// Review revision observed when the action or comment was created.
    pub review_version: i64,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Discussion text and optional final-content anchor at an expected review revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewCommentRequest {
    /// Stored text content, including Markdown where the resource contract permits it.
    pub body: String,
    /// Review revision on which the caller's decision or mutation is based.
    pub expected_review_version: i64,
    /// Final reviewed resource path to which the comment belongs.
    #[serde(default)]
    pub anchor_path: Option<String>,
    /// One-based line in the final reviewed content, when the comment is anchored.
    #[serde(default)]
    pub anchor_line: Option<i64>,
}

/// Approval or rejection and explanation for an expected review revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewDecisionRequest {
    /// Approval or rejection requested for the expected review revision.
    pub decision: ReviewDecision,
    /// Review revision on which the caller's decision or mutation is based.
    pub expected_review_version: i64,
    /// Stored text content, including Markdown where the resource contract permits it.
    pub body: Option<String>,
}

/// Approval or rejection of the submitted proposal content.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// Accept the exact submitted content fingerprint for publication.
    Approved,
    /// Reject publication and return proposals to an editable lifecycle.
    Rejected,
}

/// Expected review revision to publish atomically to its resource reference.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateReviewMergeRequest {
    /// Review revision on which the caller's decision or mutation is based.
    pub expected_review_version: i64,
}

/// Published review state and the snapshot produced by its atomic merge.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewMergeResult {
    /// Review metadata associated with these proposals or publication results.
    pub review: Review,
    /// Stable identifier of the immutable committed snapshot.
    pub commit_id: Option<String>,
    /// Number of resource operations materialized by this publication.
    pub applied_operation_count: i64,
}

/// Optional project filter for reviews visible to the authenticated member.
#[derive(Deserialize)]
pub(crate) struct ListReviewsQuery {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: Option<String>,
}
