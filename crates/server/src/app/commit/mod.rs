//! Content-addressed snapshots, commit history, and current references.

pub mod dto;
mod handler;
mod repository;
pub(crate) mod routes;
mod service;

pub(crate) mod model;

pub(crate) use repository::{
    advance_org_ref, advance_project_ref, current_org_ref, current_project_ref, load_org_ref,
    load_project_ref, lock_org_ref_for_project_projection, store_blob, validate_org_commit,
    validate_project_commit,
};

pub use service::{
    get_commit_payload, get_org_commit_state, get_project_commit_state, list_org_commits,
    list_project_commits,
};

pub(crate) use service::{create_org_commit, create_project_commit};
