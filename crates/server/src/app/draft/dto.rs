//! Request and response data for draft resources.

use crate::app::memory::dto::ResourceScope;
use crate::app::organization::dto::UserRef;
use crate::app::review::dto::Review;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    Open,
    Submitted,
    Merged,
    Discarded,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftFreshness {
    Current,
    Behind,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftReconciliationStatus {
    Unknown,
    Clean,
    Conflicts,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftCoordination {
    pub freshness: DraftFreshness,
    pub current_commit_id: Option<String>,
    pub has_upstream_resource_changes: bool,
    pub reconciliation: DraftReconciliationStatus,
    pub candidate_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftOperationAction {
    Create,
    Update,
    Rename,
    Delete,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftResourceRef {
    /// Authority namespace this proposal will target if its Review is merged.
    pub scope: ResourceScope,
    pub id: Option<String>,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftResourceContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperationInput {
    pub action: DraftOperationAction,
    pub resource: DraftResourceRef,
    pub content: Option<DraftResourceContent>,
    pub new_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperation {
    #[serde(flatten)]
    pub input: DraftOperationInput,
    pub operation_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Draft {
    pub draft_id: String,
    /// Project carrying the proposal and its pre-merge Effective Memory overlay.
    pub project_id: String,
    pub base_commit_id: Option<String>,
    pub author: UserRef,
    pub title: String,
    pub description: String,
    pub resource: DraftResourceRef,
    pub status: DraftStatus,
    pub coordination: DraftCoordination,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftSyncStatus {
    Synced,
    Pending,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftSyncState {
    pub status: DraftSyncStatus,
    pub server_cursor: Option<String>,
    pub daemon_installation_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconciliationResourceState {
    pub exists: bool,
    pub resource: DraftResourceRef,
    pub content: Option<DraftResourceContent>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationConflictKind {
    Content,
    Path,
    Existence,
    PathOccupied,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconciliationConflict {
    pub kind: ReconciliationConflictKind,
    pub field: String,
    pub base: Option<String>,
    pub current: Option<String>,
    pub draft: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationCandidateStatus {
    Clean,
    Conflicts,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftReconciliationCandidate {
    pub candidate_id: String,
    pub draft_id: String,
    pub draft_version: i64,
    pub base_commit_id: Option<String>,
    pub current_commit_id: Option<String>,
    pub status: ReconciliationCandidateStatus,
    pub base_state: ReconciliationResourceState,
    pub current_state: ReconciliationResourceState,
    pub draft_state: ReconciliationResourceState,
    pub proposed_state: Option<ReconciliationResourceState>,
    pub conflicts: Vec<ReconciliationConflict>,
    pub result_hash: Option<String>,
    pub valid: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub invalidated_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateDraftReconciliationCandidateRequest {
    pub expected_draft_version: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateDraftRebaseRequest {
    pub candidate_id: String,
    pub expected_draft_version: i64,
    pub resolved_state: Option<ReconciliationResourceState>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftRebaseResult {
    pub rebase_id: String,
    pub previous_revision_id: String,
    pub draft: DraftDetail,
    pub review: Option<Review>,
    pub approval_invalidated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftRevision {
    pub revision_id: String,
    pub draft_id: String,
    pub draft_version: i64,
    pub base_commit_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftDetail {
    pub draft: Draft,
    pub operations: Vec<DraftOperation>,
    pub sync_state: DraftSyncState,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftListResponse {
    pub items: Vec<Draft>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateDraftRequest {
    pub daemon_installation_id: String,
    pub project_id: String,
    pub base_commit_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub resource: DraftResourceRef,
    #[serde(default)]
    pub operations: Vec<DraftOperationInput>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateDraftRequest {
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftEventListResponse {
    pub events: Vec<DraftEvent>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftEvent {
    pub event_id: String,
    pub draft_id: String,
    pub project_id: String,
    pub event_type: DraftEventType,
    pub version: i64,
    pub daemon_installation_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftEventType {
    Created,
    Updated,
    OperationAppended,
    Discarded,
    Submitted,
    Reopened,
    Rebased,
    Merged,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperationBatchRequest {
    pub daemon_installation_id: String,
    pub operations: Vec<DraftOperationBatchItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperationBatchItem {
    pub local_operation_id: String,
    pub draft_id: String,
    pub expected_draft_version: i64,
    pub operation: DraftOperationInput,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftOperationBatchResponse {
    pub accepted_operations: Vec<String>,
    pub cursor: String,
}

impl DraftOperationAction {
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

#[derive(Deserialize)]
pub(crate) struct ListDraftsQuery {
    pub(super) project_id: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ListDraftEventsQuery {
    pub(super) after_cursor: Option<String>,
    pub(super) limit: Option<i64>,
}
