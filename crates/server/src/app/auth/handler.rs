//! HTTP extraction and response construction for auth resources.

use super::{AuthPrincipal, dto, service};
use crate::app::auth::dto::{OidcAuthorizationRequest, OidcCallbackRequest, TokenRequest};
use crate::http::HttpError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Adapt browser login parameters into an identity-provider authorization redirect.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn begin_oidc(
    State(state): State<AppState>,
    Query(request): Query<OidcAuthorizationRequest>,
) -> Result<Response, HttpError> {
    state.installation.require_initialized().await?;
    redirect_response(state.auth.begin_login(request).await?)
}

/// Adapt the provider callback into the native client's one-time authorization redirect.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
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

/// Exchange validated token-endpoint input for a fresh credential pair.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn exchange_auth_token(
    State(state): State<AppState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<dto::TokenResponse>, HttpError> {
    Ok(Json(state.auth.exchange_token(request).await?))
}

/// Adapt the authenticated request into session revocation and its public confirmation.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn revoke_auth_session(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::SessionRevoked>, HttpError> {
    Ok(Json(state.auth.revoke_session(&principal).await?))
}

/// Return the administrator-authorized, non-secret provider configuration.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_admin_identity_provider(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::OidcProviderStatus>, HttpError> {
    Ok(Json(state.auth.provider_status(&principal)?))
}

/// Build a redirect response only when its destination is a valid HTTP header value.
///
/// # Errors
/// Rejects a destination that cannot be encoded as an HTTP Location header.
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

/// Assemble the authenticated user's identity, organization, accessible projects, and
/// capabilities.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_me(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<dto::MeResponse>, HttpError> {
    Ok(Json(service::get_me(&state.pool, &principal).await?))
}
