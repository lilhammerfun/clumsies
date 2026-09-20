//! Projects, project membership, and project administration.

pub mod dto;
mod handler;
mod repository;
pub(crate) mod routes;
mod service;

pub(crate) mod model;

pub(crate) use repository::list_project_refs;

pub use service::{
    create_admin_project, create_admin_project_member, create_project, create_project_from_request,
    delete_admin_project, delete_admin_project_member, delete_project, get_admin_project,
    get_project, list_admin_project_members, list_admin_projects, list_project_member_candidates,
    list_project_members, list_projects, update_admin_project, update_admin_project_member,
    update_project,
};

pub(crate) use service::{ensure_project_admin, ensure_project_member};
