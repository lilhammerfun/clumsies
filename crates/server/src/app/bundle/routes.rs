//! HTTP routes for bundle resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::get;

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/me/bundles" => {
        get: handler::list_personal_bundles,
        post: handler::create_personal_bundle,
    };
    "/api/v1/me/bundles/{bundle_id}" => {
        get: handler::get_personal_bundle,
        patch: handler::update_personal_bundle,
        delete: handler::delete_personal_bundle,
    };
});
