//! Memory content, organization selections, and effective resource projections.

pub mod dto;
mod handler;
pub(crate) mod model;
mod repository;
pub(crate) mod routes;
mod service;
mod statistics;

pub(crate) use repository::{
    list_bundle_memories, load_project_org_selection, lock_org_draft_selection_coordination,
    project_org_id, user_ref,
};

pub use service::{
    create_org_context, export_memory_state, get_org_memory, get_project_memory,
    get_project_org_selection, list_org_memories, list_project_memories,
    replace_project_org_selection, select_org_resource_for_project,
};

pub(crate) use service::{
    apply_resource_operation, lock_org_draft_selection_coordination_for_project,
    pending_resource_entry, refresh_projects_for_org_resource_changes, resolve_org_resource_impact,
    select_created_org_resources_for_project, validate_project_effective_memory,
};
