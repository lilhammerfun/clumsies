//! HTTP routes for draft resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::{get, post};

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/drafts" => {
        get: handler::list_drafts,
        post: handler::create_draft,
    };
    "/api/v1/drafts/{draft_id}" => {
        get: handler::get_draft,
        patch: handler::update_draft,
        delete: handler::delete_draft,
    };
    "/api/v1/drafts/{draft_id}/operations" => {
        post: handler::append_draft_operation,
    };
    "/api/v1/drafts/{draft_id}/reconciliation-candidates" => {
        post: handler::create_draft_reconciliation_candidate,
    };
    "/api/v1/drafts/{draft_id}/reconciliation-candidates/{candidate_id}" => {
        get: handler::get_draft_reconciliation_candidate,
    };
    "/api/v1/drafts/{draft_id}/rebases" => {
        post: handler::create_draft_rebase,
    };
    "/api/v1/draft-events" => {
        get: handler::list_draft_events,
    };
    "/api/v1/draft-operation-batches" => {
        post: handler::create_draft_operation_batch,
    };
});
