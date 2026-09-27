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

/// Return public authentication capabilities.
pub(super) async fn login_methods(State(state): State<AppState>) -> Json<dto::LoginMethods> {
    Json(state.auth.login_methods())
}

/// Exchange local credentials for the same session format used by OIDC.
///
/// # Errors
/// Rejects uninitialized installations, invalid credentials and rate-limited attempts.
pub(super) async fn password_login(
    State(state): State<AppState>,
    Json(request): Json<dto::PasswordLoginRequest>,
) -> Result<Json<dto::TokenResponse>, HttpError> {
    state.installation.require_initialized().await?;
    Ok(Json(state.auth.password_login(request).await?))
}

/// Activate an admitted member using a one-time invitation.
///
/// # Errors
/// Rejects uninitialized installations and invalid or consumed invitations.
pub(super) async fn accept_invitation(
    State(state): State<AppState>,
    Json(request): Json<dto::RedeemActionRequest>,
) -> Result<Json<dto::TokenResponse>, HttpError> {
    state.installation.require_initialized().await?;
    Ok(Json(state.auth.redeem_action(request, true).await?))
}

/// Replace a password using a one-time recovery credential.
///
/// # Errors
/// Rejects invalid or consumed recovery credentials.
pub(super) async fn reset_password(
    State(state): State<AppState>,
    Json(request): Json<dto::RedeemActionRequest>,
) -> Result<Json<dto::TokenResponse>, HttpError> {
    state.installation.require_initialized().await?;
    Ok(Json(state.auth.redeem_action(request, false).await?))
}

/// Admit a member and return a credential for out-of-band delivery.
///
/// # Errors
/// Rejects insufficient administrator privileges or invalid roles.
pub(super) async fn invite(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthPrincipal>,
    Json(request): Json<dto::InvitationRequest>,
) -> Result<Json<dto::ActionTokenResponse>, HttpError> {
    Ok(Json(state.auth.invite(&actor, request).await?))
}

/// Reissue an invitation for an existing pending member.
///
/// # Errors
/// Rejects unauthorized roles and non-pending accounts.
pub(super) async fn reinvite(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthPrincipal>,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Result<Json<dto::ActionTokenResponse>, HttpError> {
    Ok(Json(
        state
            .auth
            .issue_member_action(&actor, &user_id, true)
            .await?,
    ))
}

/// Issue an administrator-approved password reset credential.
///
/// # Errors
/// Rejects unauthorized administrators or accounts without local credentials.
pub(super) async fn issue_reset(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthPrincipal>,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Result<Json<dto::ActionTokenResponse>, HttpError> {
    Ok(Json(
        state
            .auth
            .issue_member_action(&actor, &user_id, false)
            .await?,
    ))
}

/// Revoke an outstanding invitation or reset credential.
///
/// # Errors
/// Rejects insufficient administrator privileges.
pub(super) async fn revoke_action(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthPrincipal>,
    axum::extract::Path(token_id): axum::extract::Path<String>,
) -> Result<StatusCode, HttpError> {
    state.auth.revoke_action(&actor, &token_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Read configured credentials for the signed-in account.
///
/// # Errors
/// Propagates missing account or persistence failures.
pub(super) async fn account_credentials(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthPrincipal>,
) -> Result<Json<dto::AccountCredentials>, HttpError> {
    Ok(Json(state.auth.account_credentials(&actor).await?))
}

/// Change local credentials after account-control verification.
///
/// # Errors
/// Rejects stale proof and invalid credentials.
pub(super) async fn change_password(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthPrincipal>,
    Json(request): Json<dto::ChangePasswordRequest>,
) -> Result<Json<dto::TokenResponse>, HttpError> {
    Ok(Json(state.auth.change_password(&actor, request).await?))
}

/// Begin explicit OIDC binding for the authenticated account.
///
/// # Errors
/// Rejects invalid proof, missing provider or invalid callback parameters.
pub(super) async fn bind_oidc(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthPrincipal>,
    Json(request): Json<dto::BindOidcRequest>,
) -> Result<Json<dto::OidcBindingResponse>, HttpError> {
    Ok(Json(state.auth.begin_binding(&actor, request).await?))
}
