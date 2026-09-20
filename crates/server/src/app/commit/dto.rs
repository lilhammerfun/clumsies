//! Request and response data for commit resources.

use crate::app::memory::dto::ProjectOrgSelection;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Ownership scope of an immutable snapshot and its reference.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommitScope {
    /// Authoritative organization snapshot shared by selected projects.
    Org,
    /// Snapshot of a project's effective content and selected organization Memory.
    Project,
}

/// Ownership or client-configuration scope of a snapshot entry.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TreeEntryScope {
    /// Content owned by the organization.
    Org,
    /// Content owned by or configuration attached to one project.
    Project,
    /// Configuration belonging to the native runtime rather than a content owner.
    Daemon,
}

/// How a resource or configuration item entered the effective snapshot.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TreeEntrySource {
    /// Entry from authoritative organization content.
    Org,
    /// Entry originating from project-owned content.
    Project,
    /// Organization content included through an explicit project selection.
    SelectedOrg,
    /// Entry introduced by initial configuration rather than authoring.
    Bootstrap,
    /// Entry derived from runtime configuration.
    Config,
}

/// Content category determining how a snapshot entry is interpreted.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TreeEntryKind {
    /// Markdown resource content materialized into the client's Memory files.
    Memory,
    /// Project configuration describing selected organization resources.
    ProjectOrgSelection,
}

/// Immutable snapshot metadata linking a tree to its predecessor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Commit {
    /// Stable identifier of the immutable committed snapshot.
    pub commit_id: String,
    /// Ownership boundary determining which reference and resource set apply.
    pub scope: CommitScope,
    /// Organization boundary to which the resource or identity belongs.
    pub org_id: String,
    /// Project boundary containing the resource or proposal.
    pub project_id: Option<String>,
    /// Content-addressed identifier of the snapshot's materialization tree.
    pub tree_id: String,
    /// Immediate predecessor of the snapshot, absent for the first commit.
    pub parent_commit_id: Option<String>,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub version: i64,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Named reference to the latest snapshot within an ownership scope.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ref {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Ownership boundary determining which reference and resource set apply.
    pub scope: CommitScope,
    /// Organization boundary to which the resource or identity belongs.
    pub org_id: String,
    /// Project boundary containing the resource or proposal.
    pub project_id: Option<String>,
    /// Stable identifier of the immutable committed snapshot.
    pub commit_id: Option<String>,
    /// UTC timestamp of the latest persisted change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Current reference and download information for client synchronization.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitStateResponse {
    /// Whether the server head differs from the client's local commit.
    pub update_available: bool,
    /// Current named snapshot head and its update metadata.
    #[serde(rename = "ref")]
    pub reference: Ref,
    /// Metadata of the current snapshot, absent before initial publication.
    pub latest: Option<Commit>,
    /// API URL from which the referenced snapshot can be downloaded.
    pub download_url: Option<String>,
    /// Whether the advertised synchronization path supports incremental transfer.
    pub incremental_supported: bool,
}

/// One resource or configuration payload within a materialized snapshot.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreeEntry {
    /// Stable identifier of the resource described by this result.
    pub id: String,
    /// Whether the entry contains Memory or project selection configuration.
    #[serde(rename = "type")]
    pub kind: TreeEntryKind,
    /// Ownership boundary determining which reference and resource set apply.
    pub scope: TreeEntryScope,
    /// Project boundary containing the resource or proposal.
    pub project_id: Option<String>,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub path: Option<String>,
    /// Content-addressed identifier of the stored payload.
    pub blob_id: String,
    /// How this entry entered the snapshot, including selected organization content.
    pub source: TreeEntrySource,
    /// Human-readable explanation associated with the resource.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Content-addressed collection of resource and configuration entries.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tree {
    /// Content-addressed identifier of the snapshot's materialization tree.
    pub tree_id: String,
    /// Ordered resource and configuration entries making up a snapshot tree.
    pub entries: Vec<TreeEntry>,
}

/// Content-addressed payload referenced by one or more snapshot entries.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Blob {
    /// Content-addressed identifier of the stored payload.
    pub blob_id: String,
    /// Stored payload text whose bytes determine the content-addressed blob identity.
    pub content: String,
}

/// Complete snapshot download containing metadata, tree, and referenced blobs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitPayload {
    /// Snapshot metadata associated with the exported content.
    pub commit: Commit,
    /// Resource and configuration entries referenced by a snapshot.
    pub tree: Tree,
    /// Payloads referenced by the snapshot's tree entries.
    pub blobs: Vec<Blob>,
    /// Organization resources incorporated into the project's effective snapshot.
    pub project_org_selection: Option<ProjectOrgSelection>,
}

/// Ordered snapshot history with collection pagination metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<Commit>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Client snapshot identity supplied when checking for an update.
#[derive(Deserialize)]
pub(crate) struct CommitStateQuery {
    /// Snapshot already held by the client, used to determine update availability.
    pub(super) local_commit_id: Option<String>,
}
