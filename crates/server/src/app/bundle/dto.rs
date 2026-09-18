//! Request and response data for bundle resources.

use crate::app::memory::dto::MemoryMeta;
use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExportBundle {
    pub bundle_id: String,
    pub owner_user_id: String,
    pub name: String,
    pub description: String,
    pub resource_ids: Vec<String>,
    pub revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleRequest {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub resource_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleUpdateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub resource_ids: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleListResponse {
    pub items: Vec<PersonalBundleMeta>,
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleDetail {
    pub bundle: PersonalBundleMeta,
    pub memories: Vec<MemoryMeta>,
    pub etag: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonalBundleMeta {
    pub bundle_id: String,
    pub owner_user_id: String,
    pub name: String,
    pub description: String,
    pub resource_count: i64,
    pub revision: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}
