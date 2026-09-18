//! HTTP routes for audit event resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::get;

define_routes!(admin_routes, ADMIN_OPERATIONS, {
    "/api/v1/admin/audit-events" => {
        get: handler::list_admin_audit_events,
    };
});
