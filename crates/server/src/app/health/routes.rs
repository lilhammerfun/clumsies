//! HTTP routes for health resources.

use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::get;

define_routes!(public_routes, PUBLIC_OPERATIONS, {
    "/api/v1/admin/health" => {
        get: super::admin_health,
    };
});
