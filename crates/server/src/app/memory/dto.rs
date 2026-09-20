//! Request and response data for memory resources.

use crate::app::bundle::dto::MemoryExportBundle;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Neutral, verifiable export of the org's effective Memory state for the
/// Unified Memory migration: every Memory, active Draft, Project org
/// selection, and personal
/// bundles. IDs are emitted as-is so the export doubles as the
/// old_id -> memory_id identity map (identity is preserved).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExport {
    /// Organization boundary to which the resource or identity belongs.
    pub org_id: String,
    /// UTC timestamp at which the export was assembled.
    pub exported_at: String,
    /// Memory resources selected or returned by this operation.
    pub memories: Vec<MemoryExportItem>,
    /// Proposals included in the response, export, or review.
    pub drafts: Vec<MemoryExportDraft>,
    /// Project-to-organization resource selections included in the export.
    pub selections: Vec<MemoryExportSelection>,
    /// Personal collections included in the exported state.
    pub bundles: Vec<MemoryExportBundle>,
}

/// Authoritative resource content and identity preserved in a Memory export.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExportItem {
    /// Stable identifier of a Memory resource.
    pub memory_id: String,
    /// Ownership boundary determining which reference and resource set apply.
    pub scope: String,
    /// Project boundary containing the resource or proposal.
    pub project_id: Option<String>,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub path: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Human-readable explanation associated with the resource.
    pub description: String,
    /// Resource lifecycle preserved in the organization export.
    pub status: String,
    /// Fingerprint used to detect resource content changes.
    pub content_hash: String,
    /// Stored text content, including Markdown where the resource contract permits it.
    pub body: String,
    /// UTC timestamp of the latest persisted change.
    pub updated_at: String,
}

/// Proposal and operation state preserved in a Memory export.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExportDraft {
    /// Stable identifier of the editable proposal.
    pub draft_id: String,
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Human-readable summary of a proposal or review.
    pub title: String,
    /// Human-readable explanation associated with the resource.
    pub description: String,
    /// Persisted organization or project ownership category.
    pub resource_scope: String,
    /// Stable identity of the resource affected by the operation.
    pub target_id: Option<String>,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub path: Option<String>,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub version: i64,
    /// Ordered mutations applied to the proposal's base state.
    pub operations: Vec<serde_json::Value>,
}

/// Project selection membership preserved in a Memory export.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExportSelection {
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Resource identities selected or affected by this operation.
    pub resource_ids: Vec<String>,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub revision: i64,
}

/// Ownership scope determining the authoritative resource and reference.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ResourceScope {
    /// Authoritative organization Memory selectable by projects.
    Org,
    /// Legacy project-owned Memory retained for existing snapshots and explicit maintenance.
    Project,
}

/// Whether authoritative Memory participates in current snapshots.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceStatus {
    /// The resource participates in current authoritative snapshots.
    Active,
    /// The resource is retained in a legacy inactive lifecycle state.
    Deprecated,
    /// The resource is retained historically but omitted from current snapshots.
    Archived,
}

/// Organization resources selected into a project's effective Memory snapshot.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectOrgSelection {
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
    /// Memory resources selected or returned by this operation.
    pub memories: Vec<MemoryMeta>,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub revision: i64,
}

/// Complete replacement set of organization resources selected by a project.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplaceProjectOrgSelectionRequest {
    /// Resource identities selected or affected by this operation.
    #[serde(default)]
    pub resource_ids: Vec<String>,
}

/// Active scoped Memory collection with pagination metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<MemoryMeta>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Public Memory metadata together with content and its HTTP validator.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDetail {
    /// Public metadata associated with the resource payload.
    pub memory: MemoryMeta,
    /// Markdown body of this Memory resource.
    pub content: String,
    /// Quoted revision or content validator used by conditional HTTP requests.
    pub etag: String,
}

/// Public Memory identity, ownership, path, and content-change metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryMeta {
    /// Stable identifier of a Memory resource.
    pub memory_id: String,
    /// Ownership boundary determining which reference and resource set apply.
    pub scope: ResourceScope,
    /// Project boundary containing the resource or proposal.
    pub project_id: Option<String>,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub path: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Human-readable explanation associated with the resource.
    pub description: String,
    /// Fingerprint used to detect resource content changes.
    pub content_hash: String,
    /// Resource lifecycle determining participation in active snapshots.
    pub status: ResourceStatus,
    /// UTC timestamp of the latest persisted change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl ResourceScope {
    /// Return the stable string representation used by the surrounding persistence or protocol
    /// contract.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Project => "project",
        }
    }
}

/// Dashboard query; dates use the requested IANA time zone.
#[derive(Clone, Debug, Deserialize)]
pub struct MemoryStatisticsQuery {
    /// Inclusive number of calendar days, including today: 7, 30 or 90.
    pub days: u32,
    /// PostgreSQL-recognized IANA time zone, such as Asia/Shanghai.
    pub time_zone: String,
}

/// Published inventory and history; retrieval telemetry is owned by the daemon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryStatistics {
    /// Server observation time as Unix seconds.
    pub generated_at: i64,
    /// Local calendar boundaries as Unix seconds, including the exclusive last boundary.
    pub day_bounds: Vec<i64>,
    /// Start of the last 7, 30 and 90 calendar days, respectively.
    pub recency_starts: Vec<i64>,
    /// Projects visible to this principal, restricted to the requested scope.
    pub project_ids: Vec<String>,
    /// Current published documents, with no content bodies.
    pub resources: Vec<StatisticsResource>,
    /// Current published inventory count, independent of list pagination.
    pub memory_count: usize,
    /// Distinct documents added within the period.
    pub added_count: usize,
    /// Distinct documents edited or renamed within the period.
    pub updated_count: usize,
    /// Distinct documents removed within the period.
    pub deleted_count: usize,
    /// Daily closing inventory; prehistory stays unknown.
    pub days: Vec<MemoryStatisticsDay>,
    /// Distinct changed documents per daily or seven-day bucket.
    pub change_buckets: Vec<MemoryStatisticsChange>,
    /// Server-known open and conflicted drafts in visible projects.
    pub open_drafts: i64,
    /// Server-known submitted drafts in visible projects.
    pub submitted_drafts: i64,
}

/// Metadata needed for rankings and directory coverage.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatisticsResource {
    /// Stable memory identity.
    pub id: String,
    /// Current display title.
    pub title: String,
    /// Path in the published snapshot.
    pub path: String,
}

/// Inventory at the end of a local calendar day, or now for today.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryStatisticsDay {
    /// Local midnight as Unix seconds.
    pub date: i64,
    /// None means the day predates the first retained published snapshot.
    pub memory_count: Option<usize>,
}

/// Distinct changes within one chart bucket.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryStatisticsChange {
    /// Bucket start as Unix seconds.
    pub date: i64,
    /// One of added, updated or deleted.
    pub kind: String,
    /// Distinct document count.
    pub count: usize,
}
