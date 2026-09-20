//! HTTP extraction and response construction for token resources.

use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::http::HttpError;
use crate::pagination::{AdminPageQuery, parse_admin_page};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};

/// Return non-secret credential metadata after checking organization administration privileges.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn list_admin_tokens(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminPageQuery>,
) -> Result<Json<dto::AccessTokenListResponse>, HttpError> {
    let page = parse_admin_page(query)?;
    Ok(Json(
        service::list_admin_tokens(&state.pool, &principal, page.offset, page.limit).await?,
    ))
}

/// Revoke an organization credential and record the actor in the same transaction.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn delete_admin_token(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(token_id): Path<String>,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    Ok(Json(
        service::delete_admin_token(&state.pool, &principal, &token_id).await?,
    ))
}
