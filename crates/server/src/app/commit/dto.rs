//! Request and response data for commit resources.

use crate::app::memory::dto::ProjectOrgSelection;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommitScope {
    Org,
    Project,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TreeEntryScope {
    Org,
    Project,
    Daemon,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TreeEntrySource {
    Org,
    Project,
    SelectedOrg,
    Bootstrap,
    Config,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TreeEntryKind {
    Memory,
    ProjectOrgSelection,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Commit {
    pub commit_id: String,
    pub scope: CommitScope,
    pub org_id: String,
    pub project_id: Option<String>,
    pub tree_id: String,
    pub parent_commit_id: Option<String>,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub scope: CommitScope,
    pub org_id: String,
    pub project_id: Option<String>,
    pub commit_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitStateResponse {
    pub update_available: bool,
    #[serde(rename = "ref")]
    pub reference: Ref,
    pub latest: Option<Commit>,
    pub download_url: Option<String>,
    pub incremental_supported: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreeEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: TreeEntryKind,
    pub scope: TreeEntryScope,
    pub project_id: Option<String>,
    pub path: Option<String>,
    pub blob_id: String,
    pub source: TreeEntrySource,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tree {
    pub tree_id: String,
    pub entries: Vec<TreeEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Blob {
    pub blob_id: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitPayload {
    pub commit: Commit,
    pub tree: Tree,
    pub blobs: Vec<Blob>,
    pub project_org_selection: Option<ProjectOrgSelection>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitListResponse {
    pub items: Vec<Commit>,
    pub page_info: PageInfo,
}

#[derive(Deserialize)]
pub(crate) struct CommitStateQuery {
    pub(super) local_commit_id: Option<String>,
}
