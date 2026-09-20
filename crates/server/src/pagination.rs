//! Shared pagination parameters and results.

use crate::error::ServerError;
use crate::http::HttpError;
use serde::{Deserialize, Serialize};

/// Continuation cursor and information about results beyond the returned page.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageInfo {
    /// Opaque position to use when requesting the following page.
    pub next_cursor: Option<String>,
    /// Whether at least one additional result exists beyond this page.
    pub has_more: bool,
}

/// Describe a complete result collection with no continuation page.
pub(crate) fn page_info() -> PageInfo {
    PageInfo {
        next_cursor: None,
        has_more: false,
    }
}

/// Trim the look-ahead row and construct the next administrative page cursor.
pub(crate) fn admin_page<T>(mut items: Vec<T>, offset: i64, limit: i64) -> (Vec<T>, PageInfo) {
    let has_more = items.len() > limit as usize;
    if has_more {
        items.truncate(limit as usize);
    }
    let next_cursor = has_more.then(|| (offset + limit).to_string());
    (
        items,
        PageInfo {
            next_cursor,
            has_more,
        },
    )
}

/// Search text combined with administrative pagination input.
#[derive(Debug, Deserialize)]
pub(crate) struct AdminSearchQuery {
    /// Validated pagination input for the administrative listing.
    #[serde(flatten)]
    pub(crate) page: AdminPageQuery,
    /// Optional user-entered search text applied before pagination.
    pub(crate) q: Option<String>,
}

/// Untrusted cursor and page-size input from an administrative HTTP request.
#[derive(Debug, Deserialize)]
pub(crate) struct AdminPageQuery {
    /// Maximum results requested for this page, subject to API bounds.
    pub(crate) limit: Option<String>,
    /// Opaque server position used to resume synchronization or pagination.
    pub(crate) cursor: Option<String>,
}

/// Validated offset and bounded page size for administrative queries.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AdminPage {
    /// Zero-based result offset after cursor validation.
    pub(crate) offset: i64,
    /// Maximum results requested for this page, subject to API bounds.
    pub(crate) limit: i64,
}

/// Decode the cursor and validate the administrative page-size bounds.
///
/// # Errors
/// Rejects malformed cursors and limits outside the supported administrative page bounds.
pub(crate) fn parse_admin_page(query: AdminPageQuery) -> Result<AdminPage, HttpError> {
    let limit = match query.limit {
        Some(limit) => limit.parse::<i64>().map_err(|_| {
            HttpError::from(ServerError::InvalidRequest(
                "invalid admin page limit".to_owned(),
            ))
        })?,
        None => 50,
    };
    if !(1..=200).contains(&limit) {
        return Err(ServerError::InvalidRequest(
            "admin page limit must be between 1 and 200".to_owned(),
        )
        .into());
    }
    let offset = match query.cursor {
        Some(cursor) => cursor.parse::<i64>().map_err(|_| {
            HttpError::from(ServerError::InvalidRequest(
                "invalid admin page cursor".to_owned(),
            ))
        })?,
        None => 0,
    };
    if offset < 0 {
        return Err(ServerError::InvalidRequest("invalid admin page cursor".to_owned()).into());
    }
    Ok(AdminPage { offset, limit })
}
