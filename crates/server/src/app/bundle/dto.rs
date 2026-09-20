//! Request and response data for bundle resources.

use crate::app::memory::dto::MemoryMeta;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Personal collection preserved in an organization Memory export.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExportBundle {
    /// Stable identifier of the personal Memory collection.
    pub bundle_id: String,
    /// Identity that owns the personal collection.
    pub owner_user_id: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Human-readable explanation associated with the resource.
    pub description: String,
    /// Resource identities selected or affected by this operation.
    pub resource_ids: Vec<String>,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub revision: i64,
}

/// Name, description, and selected resource identities for a personal collection.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleRequest {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Human-readable explanation associated with the resource.
    pub description: Option<String>,
    /// Resource identities selected or affected by this operation.
    #[serde(default)]
    pub resource_ids: Vec<String>,
}

/// Optional collection edits; omitted resource IDs preserve the existing selection.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleUpdateRequest {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: Option<String>,
    /// Human-readable explanation associated with the resource.
    pub description: Option<String>,
    /// Resource identities selected or affected by this operation.
    pub resource_ids: Option<Vec<String>>,
}

/// Owner-visible Memory collection list with pagination metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<PersonalBundleMeta>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

/// Owner-visible collection metadata, selected memories, and revision validator.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleDetail {
    /// Personal collection metadata associated with these selected memories.
    pub bundle: PersonalBundleMeta,
    /// Memory resources selected or returned by this operation.
    pub memories: Vec<MemoryMeta>,
    /// Quoted revision or content validator used by conditional HTTP requests.
    pub etag: String,
}

/// Public personal collection metadata including its resource count.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleMeta {
    /// Stable identifier of the personal Memory collection.
    pub bundle_id: String,
    /// Identity that owns the personal collection.
    pub owner_user_id: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub name: String,
    /// Human-readable explanation associated with the resource.
    pub description: String,
    /// Number of active Memory resources included in the collection.
    pub resource_count: i64,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub revision: i64,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// UTC timestamp of the latest persisted change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}
