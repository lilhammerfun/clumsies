//! Recorded organization actions and the administrator audit feed.

pub mod dto;
mod handler;
mod repository;
pub(crate) mod routes;
mod service;

pub(crate) use repository::{insert_audit_event, insert_audit_event_with_changes};

pub use service::list_admin_audit_events;

/// Build details from a caller-owned allowlist, excluding unchanged fields.
pub(crate) fn changes(fields: &[(&str, &str, &str)]) -> Vec<dto::AuditChange> {
    fields
        .iter()
        .filter(|(_, before, after)| before != after)
        .map(|(field, before, after)| dto::AuditChange {
            field: (*field).to_owned(),
            before: (*before).to_owned(),
            after: (*after).to_owned(),
        })
        .collect()
}
