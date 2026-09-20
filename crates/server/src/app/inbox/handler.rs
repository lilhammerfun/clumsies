//! Authenticated Inbox HTTP adapters.
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::{Value, json};

use super::{
    dto::{InboxListResponse, InboxQuery, UpdateInboxRequest},
    service,
};
use crate::{app::auth::AuthPrincipal, http::HttpError, state::AppState};

/// Lists only the principal's currently accessible notifications.
///
/// # Errors
/// Returns a protocol error for invalid pagination or database failure.
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<InboxQuery>,
) -> Result<Json<InboxListResponse>, HttpError> {
    Ok(Json(
        service::list(
            &state.pool,
            &principal,
            query.cursor.as_deref(),
            query.limit,
        )
        .await?,
    ))
}

/// Persists a receipt without acknowledging events newer than the displayed revision.
///
/// # Errors
/// Inaccessible subjects return not found; invalid revisions and database writes fail explicitly.
pub(super) async fn update(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(id): Path<String>,
    Json(request): Json<UpdateInboxRequest>,
) -> Result<Json<Value>, HttpError> {
    service::update(&state.pool, &principal, &id, request).await?;
    Ok(Json(json!({"updated": true})))
}
