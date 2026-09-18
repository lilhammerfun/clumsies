//! HTTP routes for token resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::{delete, get};

define_routes!(admin_routes, ADMIN_OPERATIONS, {
    "/api/v1/admin/tokens" => {
        get: handler::list_admin_tokens,
    };
    "/api/v1/admin/tokens/{token_id}" => {
        delete: handler::delete_admin_token,
    };
});
