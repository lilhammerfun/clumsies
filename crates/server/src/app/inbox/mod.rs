//! Personal notifications committed with their source event; source content requires current access.

mod account;
pub mod dto;
mod handler;
mod repository;
pub(crate) mod routes;
mod service;

pub(crate) use account::{notify_access_change, notify_welcome};
pub(crate) use repository::{notify_draft_conflict, notify_review, notify_shared_update};
pub use service::{list, update};
