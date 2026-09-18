//! HTTP routes for project resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::{get, patch};

define_routes!(admin_routes, ADMIN_OPERATIONS, {
    "/api/v1/admin/projects" => {
        get: handler::list_admin_projects,
        post: handler::create_admin_project,
    };
});

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/projects" => {
        get: handler::list_projects,
        post: handler::create_project,
    };
    "/api/v1/projects/{project_id}" => {
        get: handler::get_project,
        patch: handler::update_project,
        delete: handler::delete_project,
    };
    "/api/v1/admin/projects/{project_id}" => {
        get: handler::get_admin_project,
        patch: handler::update_admin_project,
        delete: handler::delete_admin_project,
    };
    "/api/v1/admin/projects/{project_id}/member-candidates" => {
        get: handler::list_project_member_candidates,
    };
    "/api/v1/admin/projects/{project_id}/members" => {
        get: handler::list_admin_project_members,
        post: handler::create_admin_project_member,
    };
    "/api/v1/admin/projects/{project_id}/members/{user_id}" => {
        patch: handler::update_admin_project_member,
        delete: handler::delete_admin_project_member,
    };
    "/api/v1/projects/{project_id}/members" => {
        get: handler::list_project_members,
    };
});
