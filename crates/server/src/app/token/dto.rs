//! Request and response data for token resources.

use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Credential category used to distinguish native and integration access.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessTokenKind {
    /// Short-lived bearer credential for API requests.
    Access,
    /// Longer-lived credential consumed by refresh rotation.
    Refresh,
    /// Credential issued for an external integration.
    Integration,
    /// Credential representing a browser session.
    WebSession,
}

/// Non-secret metadata identifying an issued or revoked credential.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessTokenMeta {
    /// Stable identifier of the access or refresh credential record.
    pub token_id: String,
    /// Stable identity of the user represented or targeted by this record.
    pub user_id: String,
    /// Credential category; the secret itself is never included.
    pub kind: AccessTokenKind,
    /// Whether the requested credentials or session have been invalidated.
    pub revoked: bool,
    /// UTC deadline after which this credential or session is invalid.
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Administrative credential page with continuation metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessTokenListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<AccessTokenMeta>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}
