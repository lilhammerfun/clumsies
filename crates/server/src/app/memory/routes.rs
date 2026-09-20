//! HTTP routes for memory resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::get;

define_routes!(admin_routes, ADMIN_OPERATIONS, {
    "/api/v1/admin/memory-export" => {
        get: handler::export_org_memory_state,
    };
});

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/org/memory-statistics" => { get: handler::org_memory_statistics };
    "/api/v1/projects/{project_id}/memory-statistics" => { get: handler::project_memory_statistics };
    "/api/v1/org/memories" => {
        get: handler::list_org_memories,
    };
    "/api/v1/org/memories/{memory_id}" => {
        get: handler::get_org_memory,
    };
    "/api/v1/projects/{project_id}/memories" => {
        get: handler::list_project_memories,
    };
    "/api/v1/projects/{project_id}/memories/{memory_id}" => {
        get: handler::get_project_memory,
    };
    "/api/v1/projects/{project_id}/org-selections" => {
        get: handler::get_project_org_selection,
        put: handler::replace_project_org_selection,
    };
});
