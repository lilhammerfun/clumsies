//! HTTP routes for review resources.

use super::handler;
use crate::routes::define_routes;
use crate::state::AppState;
use axum::Router;
use axum::routing::{get, post};

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/reviews" => {
        get: handler::list_reviews,
        post: handler::create_review,
    };
    "/api/v1/reviews/{review_id}" => {
        get: handler::get_review,
    };
    "/api/v1/reviews/{review_id}/comments" => {
        get: handler::list_review_comments,
        post: handler::create_review_comment,
    };
    "/api/v1/reviews/{review_id}/decisions" => {
        post: handler::create_review_decision,
    };
    "/api/v1/reviews/{review_id}/submissions" => {
        post: handler::create_review_submission,
    };
    "/api/v1/reviews/{review_id}/org-contribution" => {
        post: handler::retry_org_contribution,
    };
    "/api/v1/reviews/{review_id}/merges" => {
        post: handler::create_review_merge,
    };
    "/api/v1/reviews/{review_id}/update-plans" => {
        post: handler::create_review_update_plan,
    };
    "/api/v1/reviews/{review_id}/auto-rebases" => {
        post: handler::create_review_auto_rebase,
    };
    "/api/v1/reviews/{review_id}/updates" => {
        post: handler::create_review_update,
    };
});
