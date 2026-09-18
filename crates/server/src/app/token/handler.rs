//! HTTP extraction and response construction for token resources.

use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::http::{HttpError, require_org_admin};
use crate::pagination::{AdminPageQuery, parse_admin_page};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};

pub(super) async fn list_admin_tokens(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminPageQuery>,
) -> Result<Json<dto::AccessTokenListResponse>, HttpError> {
    require_org_admin(&principal)?;
    let page = parse_admin_page(query)?;
    Ok(Json(
        service::list_admin_tokens(&state.pool, &principal.org_id, page.offset, page.limit).await?,
    ))
}

pub(super) async fn delete_admin_token(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Path(token_id): Path<String>,
) -> Result<Json<crate::dto::DeleteResult>, HttpError> {
    require_org_admin(&principal)?;
    Ok(Json(
        service::delete_admin_token(&state.pool, &principal, &token_id).await?,
    ))
}
