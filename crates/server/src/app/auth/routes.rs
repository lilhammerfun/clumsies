//! HTTP routes for auth resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::{delete, get, post};

define_routes!(public_routes, PUBLIC_OPERATIONS, {
    "/oauth2/authorization/oidc" => {
        get: handler::begin_oidc,
    };
    "/login/oauth2/code/oidc" => {
        get: handler::complete_oidc,
    };
    "/api/v1/auth/token" => {
        post: handler::exchange_auth_token,
    };
});

define_routes!(admin_routes, ADMIN_OPERATIONS, {
    "/api/v1/admin/identity-provider" => {
        get: handler::get_admin_identity_provider,
    };
});

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/auth/session" => {
        delete: handler::revoke_auth_session,
    };
    "/api/v1/me" => {
        get: handler::get_me,
    };
});
