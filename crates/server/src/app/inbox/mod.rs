//! Personal notifications produced with their source transaction and scoped by current access.

pub mod dto;
mod handler;
mod repository;
pub(crate) mod routes;
mod service;

pub(crate) use repository::{notify_review, notify_shared_update};
pub use service::{list, update};
