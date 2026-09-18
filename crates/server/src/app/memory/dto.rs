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
    pub org_id: String,
    pub exported_at: String,
    pub memories: Vec<MemoryExportItem>,
    pub drafts: Vec<MemoryExportDraft>,
    pub selections: Vec<MemoryExportSelection>,
    pub bundles: Vec<MemoryExportBundle>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExportItem {
    pub memory_id: String,
    pub scope: String,
    pub project_id: Option<String>,
    pub path: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub content_hash: String,
    pub body: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExportDraft {
    pub draft_id: String,
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub resource_scope: String,
    pub target_id: Option<String>,
    pub path: Option<String>,
    pub status: String,
    pub version: i64,
    pub operations: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExportSelection {
    pub project_id: String,
    pub resource_ids: Vec<String>,
    pub revision: i64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ResourceScope {
    Org,
    Project,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceStatus {
    Active,
    Deprecated,
    Archived,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectOrgSelection {
    pub project_id: String,
    pub memories: Vec<MemoryMeta>,
    pub revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplaceProjectOrgSelectionRequest {
    #[serde(default)]
    pub resource_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryListResponse {
    pub items: Vec<MemoryMeta>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDetail {
    pub memory: MemoryMeta,
    pub content: String,
    pub etag: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryMeta {
    pub memory_id: String,
    pub scope: ResourceScope,
    pub project_id: Option<String>,
    pub path: String,
    pub name: String,
    pub description: String,
    pub content_hash: String,
    pub status: ResourceStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl ResourceScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Project => "project",
        }
    }
}
