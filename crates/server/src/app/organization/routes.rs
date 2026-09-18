//! HTTP routes for organization resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::{get, patch};

define_routes!(admin_routes, ADMIN_OPERATIONS, {
    "/api/v1/admin/org" => {
        get: handler::get_admin_org,
        patch: handler::update_admin_org,
    };
    "/api/v1/admin/members" => {
        get: handler::list_admin_members,
        post: handler::create_admin_member,
    };
    "/api/v1/admin/members/{user_id}" => {
        patch: handler::update_admin_member,
        delete: handler::delete_admin_member,
    };
});
