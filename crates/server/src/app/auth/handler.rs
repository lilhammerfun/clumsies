//! HTTP extraction and response construction for auth resources.

use super::{AuthPrincipal, dto, service};
use crate::app::auth::dto::{OidcAuthorizationRequest, OidcCallbackRequest, TokenRequest};
use crate::http::{HttpError, require_org_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

pub(super) async fn begin_oidc(
    State(state): State<AppState>,
    Query(request): Query<OidcAuthorizationRequest>,
) -> Result<Response, HttpError> {
    state.installation.require_initialized().await?;
    redirect_response(state.auth.begin_login(request).await?)
}

pub(super) async fn complete_oidc(
    State(state): State<AppState>,
    Query(request): Query<OidcCallbackRequest>,
) -> Result<Response, HttpError> {
    let redirect_uri = state
        .auth
        .complete_login(request, &state.installation)
        .await?;
    redirect_response(redirect_uri)
}

pub(super) async fn exchange_auth_token(
    State(state): State<AppState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<dto::TokenResponse>, HttpError> {
    Ok(Json(state.auth.exchange_token(request).await?))
}

pub(super) async fn revoke_auth_session(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::SessionRevoked>, HttpError> {
    Ok(Json(state.auth.revoke_session(&principal).await?))
}

pub(super) async fn get_admin_identity_provider(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::OidcProviderStatus>, HttpError> {
    require_org_admin(&principal)?;
    Ok(Json(state.auth.provider_status()))
}

fn redirect_response(location: String) -> Result<Response, HttpError> {
    let location = HeaderValue::from_str(&location)
        .map_err(|_| HttpError::bad_request("redirect URL produced an invalid Location header"))?;
    Ok((
        StatusCode::FOUND,
        [
            (LOCATION, location),
            (CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
    )
        .into_response())
}

pub(super) async fn get_me(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::MeResponse>, HttpError> {
    Ok(Json(service::get_me(&state.pool, &principal).await?))
}
