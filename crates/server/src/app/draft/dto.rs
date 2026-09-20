//! Request and response data for draft resources.

use crate::app::memory::dto::ResourceScope;
use crate::app::organization::dto::UserRef;
use crate::app::review::dto::Review;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Lifecycle governing whether a proposal may be edited, reviewed, or published.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    /// The author may edit or submit the proposal.
    Open,
    /// The proposal is participating in an active review.
    Submitted,
    /// The proposal's changes were published and its lifecycle is complete.
    Merged,
    /// The proposal was abandoned and no longer accepts changes.
    Discarded,
}

/// Whether the proposal's ancestor still matches the upstream reference.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftFreshness {
    /// The proposal ancestor matches its current authoritative reference.
    Current,
    /// The proposal ancestor differs from its current authoritative reference.
    Behind,
}

/// Availability and conflict state of the proposal's current candidate.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftReconciliationStatus {
    /// No valid evidence exists for the current proposal and upstream revisions.
    Unknown,
    /// Current evidence can reconcile without manual conflict resolution.
    Clean,
    /// Current evidence requires explicit resolution of conflicting state.
    Conflicts,
}

/// Freshness and candidate availability relative to the proposal's current reference.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftCoordination {
    /// Relationship between the proposal's base commit and the current reference.
    pub freshness: DraftFreshness,
    /// Reference head observed when computing freshness or reconciliation.
    pub current_commit_id: Option<String>,
    /// Whether upstream changed this resource since the proposal's base.
    pub has_upstream_resource_changes: bool,
    /// Availability of a clean or conflicting reconciliation candidate.
    pub reconciliation: DraftReconciliationStatus,
    /// Identifier of the reconciliation result being inspected or applied.
    pub candidate_id: Option<String>,
}

/// Mutation represented by a proposal operation.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftOperationAction {
    /// Introduce a new resource at the supplied path.
    Create,
    /// Replace content of the existing target resource.
    Update,
    /// Move the existing resource to a new path while preserving its identity.
    Rename,
    /// Remove the target resource from active authoritative content.
    Delete,
}

/// Stable resource identity or creation path within an ownership scope.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftResourceRef {
    /// Authority namespace this proposal will target if its Review is merged.
    pub scope: ResourceScope,
    /// Stable identifier of the resource described by this result.
    pub id: Option<String>,
    /// Optional resource path used for creation or resolving a target without an explicit
    /// identity.
    pub path: Option<String>,
}

/// Editable Markdown payload and optional descriptive metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftResourceContent {
    /// Human-readable explanation associated with the resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Markdown body of this Memory resource.
    pub content: String,
}

/// Resource mutation before assignment of its server identity and timestamp.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperationInput {
    /// Mutation to apply to the referenced resource.
    pub action: DraftOperationAction,
    /// Stable resource identity and scope targeted by the operation.
    pub resource: DraftResourceRef,
    /// Optional Memory payload for create or update; rename and delete omit content.
    pub content: Option<DraftResourceContent>,
    /// Requested destination for a rename, validated for portability before persistence.
    pub new_path: Option<String>,
}

/// Server-identified mutation in the proposal's stable operation order.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperation {
    /// Validated mutation data associated with a persisted operation.
    #[serde(flatten)]
    pub input: DraftOperationInput,
    /// Server identifier of a persisted draft operation.
    pub operation_id: String,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Editable resource proposal with lifecycle, revision, and upstream coordination state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Draft {
    /// Stable identifier of the editable proposal.
    pub draft_id: String,
    /// Project carrying the proposal and its pre-merge Effective Memory overlay.
    pub project_id: String,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub base_commit_id: Option<String>,
    /// Public identity of the author of this record.
    pub author: UserRef,
    /// Human-readable summary of a proposal or review.
    pub title: String,
    /// Human-readable explanation associated with the resource.
    pub description: String,
    /// Stable resource identity and scope targeted by the operation.
    pub resource: DraftResourceRef,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub status: DraftStatus,
    /// Freshness and reconciliation information relative to the current reference.
    pub coordination: DraftCoordination,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub version: i64,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// UTC timestamp of the latest persisted change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Server acknowledgment state of a client proposal.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftSyncStatus {
    /// The server has acknowledged the represented client changes.
    Synced,
    /// Client changes are awaiting server acknowledgment.
    Pending,
    /// The client reports that synchronization did not complete.
    Failed,
}

/// Server acknowledgment used to correlate synchronization with a client installation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftSyncState {
    /// Whether the represented client changes have been acknowledged by the server.
    pub status: DraftSyncStatus,
    /// Last synchronization position acknowledged by the server.
    pub server_cursor: Option<String>,
    /// Client installation identity used to correlate draft synchronization events.
    pub daemon_installation_id: Option<String>,
}

/// Existence, identity, and content of one resource at a reconciliation point.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconciliationResourceState {
    /// Whether the resource exists at the represented revision.
    pub exists: bool,
    /// Stable resource identity and scope targeted by the operation.
    pub resource: DraftResourceRef,
    /// Optional Memory payload for create or update; rename and delete omit content.
    pub content: Option<DraftResourceContent>,
}

/// Resource dimension preventing automatic three-way reconciliation.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationConflictKind {
    /// Both sides changed content incompatibly relative to their ancestor.
    Content,
    /// Both sides renamed the same resource incompatibly.
    Path,
    /// Creation or deletion conflicts with the other side's resource state.
    Existence,
    /// Another active resource already occupies the resulting path.
    PathOccupied,
}

/// Field-level disagreement that requires an explicit user resolution.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconciliationConflict {
    /// Dimension of the resource state that requires explicit conflict resolution.
    pub kind: ReconciliationConflictKind,
    /// Resource field whose competing changes cannot be reconciled automatically.
    pub field: String,
    /// Ancestor value used in three-way conflict detection.
    pub base: Option<String>,
    /// Upstream value used in three-way conflict detection.
    pub current: Option<String>,
    /// Proposal metadata and state associated with this response.
    pub draft: Option<String>,
}

/// Whether three-way reconciliation produced a usable automatic result.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationCandidateStatus {
    /// The candidate contains a complete automatic reconciliation result.
    Clean,
    /// The candidate records fields requiring user-supplied resolution.
    Conflicts,
}

/// Persisted three-way reconciliation tied to exact draft and upstream revisions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftReconciliationCandidate {
    /// Identifier of the reconciliation result being inspected or applied.
    pub candidate_id: String,
    /// Stable identifier of the editable proposal.
    pub draft_id: String,
    /// Proposal revision to which this record or candidate applies.
    pub draft_version: i64,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub base_commit_id: Option<String>,
    /// Reference head observed when computing freshness or reconciliation.
    pub current_commit_id: Option<String>,
    /// Whether this evidence contains a clean result or unresolved conflicts.
    pub status: ReconciliationCandidateStatus,
    /// Resource state at the proposal's ancestor commit.
    pub base_state: ReconciliationResourceState,
    /// Resource state at the upstream reference head.
    pub current_state: ReconciliationResourceState,
    /// Resource state produced by applying the proposal to its ancestor.
    pub draft_state: ReconciliationResourceState,
    /// Automatically reconciled result, absent when conflicts require user resolution.
    pub proposed_state: Option<ReconciliationResourceState>,
    /// Unresolved differences between ancestor, upstream, and proposed content.
    pub conflicts: Vec<ReconciliationConflict>,
    /// Fingerprint of materialized content used to validate reconciliation or approval.
    pub result_hash: Option<String>,
    /// Whether the candidate still matches the proposal version, base, and current reference.
    pub valid: bool,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// UTC time at which a reconciliation candidate ceased to be usable.
    #[serde(with = "time::serde::rfc3339::option")]
    pub invalidated_at: Option<OffsetDateTime>,
}

/// Draft revision against which to compute a reusable reconciliation candidate.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateDraftReconciliationCandidateRequest {
    /// Proposal revision on which the caller's mutation is based.
    pub expected_draft_version: i64,
}

/// Candidate and optional conflict resolution to apply at an expected draft revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateDraftRebaseRequest {
    /// Identifier of the reconciliation result being inspected or applied.
    pub candidate_id: String,
    /// Proposal revision on which the caller's mutation is based.
    pub expected_draft_version: i64,
    /// User-supplied final state for a conflicting reconciliation candidate.
    pub resolved_state: Option<ReconciliationResourceState>,
}

/// Applied reconciliation, saved prior revision, and resulting approval state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftRebaseResult {
    /// Stable identifier of the recorded reconciliation application.
    pub rebase_id: String,
    /// Saved draft revision that permits auditing the state before rebase.
    pub previous_revision_id: String,
    /// Proposal metadata and state associated with this response.
    pub draft: DraftDetail,
    /// Review metadata associated with these proposals or publication results.
    pub review: Option<Review>,
    /// Whether rebasing changed content covered by an existing approval.
    pub approval_invalidated: bool,
}

/// Saved legacy proposal state required to preserve migration semantics.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftRevision {
    /// Identifier of an immutable saved draft revision.
    pub revision_id: String,
    /// Stable identifier of the editable proposal.
    pub draft_id: String,
    /// Proposal revision to which this record or candidate applies.
    pub draft_version: i64,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub base_commit_id: Option<String>,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Proposal metadata, ordered mutations, and acknowledged client synchronization state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftDetail {
    /// Proposal metadata and state associated with this response.
    pub draft: Draft,
    /// Ordered mutations applied to the proposal's base state.
    pub operations: Vec<DraftOperation>,
    /// Acknowledged server state of the client's proposal synchronization.
    pub sync_state: DraftSyncState,
}

/// Author-visible proposal collection with pagination metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<Draft>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Resource proposal, ordered initial mutations, and its ancestor commit.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateDraftRequest {
    /// Client installation identity used to correlate draft synchronization events.
    pub daemon_installation_id: String,
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub base_commit_id: Option<String>,
    /// Human-readable summary of a proposal or review.
    pub title: String,
    /// Human-readable explanation associated with the resource.
    pub description: Option<String>,
    /// Stable resource identity and scope targeted by the operation.
    pub resource: DraftResourceRef,
    /// Ordered mutations applied to the proposal's base state.
    #[serde(default)]
    pub operations: Vec<DraftOperationInput>,
}

/// Optional proposal title and description edits at an expected revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateDraftRequest {
    /// Human-readable summary of a proposal or review.
    pub title: Option<String>,
    /// Human-readable explanation associated with the resource.
    pub description: Option<String>,
}

/// Ordered proposal lifecycle events with the cursor for the next synchronization request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftEventListResponse {
    /// Persisted lifecycle events included in the synchronization response.
    pub events: Vec<DraftEvent>,
    /// Opaque position to use when requesting the following page.
    pub next_cursor: Option<String>,
    /// Whether at least one additional result exists beyond this page.
    pub has_more: bool,
}

/// Persisted lifecycle notification used to resume proposal synchronization.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftEvent {
    /// Stable opaque event identity; ordering uses the separate synchronization cursor.
    pub event_id: String,
    /// Stable identifier of the editable proposal.
    pub draft_id: String,
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Lifecycle change represented by this event.
    pub event_type: DraftEventType,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub version: i64,
    /// Client installation identity used to correlate draft synchronization events.
    pub daemon_installation_id: Option<String>,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Persisted proposal lifecycle changes observable by synchronization clients.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftEventType {
    /// A new proposal and its initial mutations were persisted.
    Created,
    /// Proposal metadata changed without adding a resource mutation.
    Updated,
    /// The ordered mutation sequence gained a new operation.
    OperationAppended,
    /// An editable proposal was abandoned without publication.
    Discarded,
    /// The proposal entered the review lifecycle.
    Submitted,
    /// A submitted proposal became editable after rejection.
    Reopened,
    /// The proposal ancestor and mutations were reconciled with upstream state.
    Rebased,
    /// Reviewed proposal content was published to its authoritative reference.
    Merged,
}

/// Ordered client mutations accepted atomically across their affected drafts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperationBatchRequest {
    /// Client installation identity used to correlate draft synchronization events.
    pub daemon_installation_id: String,
    /// Ordered mutations applied to the proposal's base state.
    pub operations: Vec<DraftOperationBatchItem>,
}

/// Client mutation identity and expected draft revision for one batch entry.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperationBatchItem {
    /// Client-generated identity acknowledged when a batch item is accepted.
    pub local_operation_id: String,
    /// Stable identifier of the editable proposal.
    pub draft_id: String,
    /// Proposal revision on which the caller's mutation is based.
    pub expected_draft_version: i64,
    /// Mutation carried by this batch entry or migration overlay.
    pub operation: DraftOperationInput,
}

/// Acknowledged local operation identities and the final persisted event cursor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperationBatchResponse {
    /// Local operation IDs durably accepted by the server.
    pub accepted_operations: Vec<String>,
    /// Opaque server position used to resume synchronization or pagination.
    pub cursor: String,
}

impl DraftOperationAction {
    /// Return the stable string representation used by the surrounding persistence or protocol
    /// contract.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Rename => "rename",
            Self::Delete => "delete",
        }
    }
}

impl DraftEventType {
    /// Return the stable string representation used by the surrounding persistence or protocol
    /// contract.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::OperationAppended => "operation_appended",
            Self::Discarded => "discarded",
            Self::Submitted => "submitted",
            Self::Reopened => "reopened",
            Self::Rebased => "rebased",
            Self::Merged => "merged",
        }
    }
}

/// Optional project filter for the authenticated author's proposals.
#[derive(Deserialize)]
pub(crate) struct ListDraftsQuery {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: Option<String>,
}

/// Exclusive synchronization cursor and bounded event count.
#[derive(Deserialize)]
pub(crate) struct ListDraftEventsQuery {
    /// Exclusive event position from which to resume reading.
    pub(super) after_cursor: Option<String>,
    /// Maximum results requested for this page, subject to API bounds.
    pub(super) limit: Option<i64>,
}
