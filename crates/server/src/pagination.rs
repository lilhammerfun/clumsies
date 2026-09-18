//! Shared pagination parameters and results.

use crate::error::ServerError;
use crate::http::HttpError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageInfo {
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

pub(crate) fn page_info() -> PageInfo {
    PageInfo {
        next_cursor: None,
        has_more: false,
    }
}

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

#[derive(Debug, Deserialize)]
pub(crate) struct AdminSearchQuery {
    #[serde(flatten)]
    pub(crate) page: AdminPageQuery,
    pub(crate) q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminPageQuery {
    pub(crate) limit: Option<String>,
    pub(crate) cursor: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AdminPage {
    pub(crate) offset: i64,
    pub(crate) limit: i64,
}

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
