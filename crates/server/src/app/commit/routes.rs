//! HTTP routes for commit resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::get;

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/org/commits" => {
        get: handler::list_org_commits,
    };
    "/api/v1/org/commit-state" => {
        get: handler::get_org_commit_state,
    };
    "/api/v1/projects/{project_id}/commits" => {
        get: handler::list_project_commits,
    };
    "/api/v1/projects/{project_id}/commit-state" => {
        get: handler::get_project_commit_state,
    };
    "/api/v1/commits/{commit_id}" => {
        get: handler::get_commit,
    };
});
