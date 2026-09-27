//! HTTP routes for auth resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::{delete, get, post};

define_routes!(public_routes, PUBLIC_OPERATIONS, {
    "/api/v1/auth/methods" => { get: handler::login_methods, };
    "/api/v1/auth/password/sessions" => { post: handler::password_login, };
    "/api/v1/auth/invitations/accept" => { post: handler::accept_invitation, };
    "/api/v1/auth/password/reset" => { post: handler::reset_password, };
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
    "/api/v1/admin/invitations" => { post: handler::invite, };
    "/api/v1/admin/members/{user_id}/invitation" => { post: handler::reinvite, };
    "/api/v1/admin/members/{user_id}/password-reset" => { post: handler::issue_reset, };
    "/api/v1/admin/action-tokens/{token_id}" => { delete: handler::revoke_action, };
    "/api/v1/admin/identity-provider" => {
        get: handler::get_admin_identity_provider,
    };
});

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/auth/credentials" => { get: handler::account_credentials, };
    "/api/v1/auth/password" => { post: handler::change_password, };
    "/api/v1/auth/oidc-bindings" => { post: handler::bind_oidc, };
    "/api/v1/auth/session" => {
        delete: handler::revoke_auth_session,
    };
    "/api/v1/me" => {
        get: handler::get_me,
    };
});
