//! Authenticated routes for personal notifications and receipts.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::{get, patch};

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/me/inbox" => { get: handler::list };
    "/api/v1/me/inbox/{notification_id}" => { patch: handler::update };
});
