//! HTTP routes for installation resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::{get, post, put};

define_routes!(public_routes, PUBLIC_OPERATIONS, {
    "/api/v1/setup" => {
        get: handler::get_setup,
    };
    "/api/v1/setup/sessions" => {
        post: handler::create_setup_session,
    };
    "/api/v1/setup/configuration" => {
        put: handler::replace_setup_configuration,
    };
    "/api/v1/setup/oidc-authorizations" => {
        post: handler::create_setup_oidc_authorization,
    };
});
