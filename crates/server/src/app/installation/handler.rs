//! HTTP extraction and response construction for installation resources.

use super::{InstallationError, InstallationService, dto};
use crate::app::installation::dto::{
    CreateSetupSessionRequest, ReplaceSetupConfigurationRequest, SetupOidcAuthorization,
    SetupOidcAuthorizationRequest,
};
use crate::http::{HttpError, cookie_value};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use cookie::{Cookie, SameSite};

/// Return the current installation phase and setup capabilities.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn get_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<dto::SetupStatus>, HttpError> {
    let session_token = setup_session_token(&headers, state.installation.cookie_name());
    Ok(Json(
        state
            .installation
            .status(session_token.as_deref(), state.auth.configured())
            .await?,
    ))
}

/// Validate the bootstrap secret and issue a short-lived setup cookie.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_setup_session(
    State(state): State<AppState>,
    Json(request): Json<CreateSetupSessionRequest>,
) -> Result<Response, HttpError> {
    let credentials = state
        .installation
        .create_session(&request.setup_code)
        .await?;
    let cookie = Cookie::build((
        state.installation.cookie_name().to_owned(),
        credentials.token,
    ))
    .path("/")
    .http_only(true)
    .secure(state.installation.cookie_secure())
    .same_site(SameSite::Strict)
    .max_age(cookie::time::Duration::minutes(15))
    .build();
    let mut response = (StatusCode::CREATED, Json(credentials.session)).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie.to_string())
            .map_err(|_| HttpError::internal("setup cookie contains an invalid value"))?,
    );
    Ok(response)
}

/// Replace staged setup settings after validating the session and CSRF proof.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn replace_setup_configuration(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ReplaceSetupConfigurationRequest>,
) -> Result<Json<dto::SetupConfiguration>, HttpError> {
    let (session_token, csrf_token) = setup_credentials(&state.installation, &headers)?;
    Ok(Json(
        state
            .installation
            .replace_configuration(&session_token, &csrf_token, request)
            .await?,
    ))
}

/// Authorize first-owner browser login using the active setup session.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
pub(super) async fn create_setup_oidc_authorization(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SetupOidcAuthorizationRequest>,
) -> Result<(StatusCode, Json<SetupOidcAuthorization>), HttpError> {
    let (session_token, csrf_token) = setup_credentials(&state.installation, &headers)?;
    let setup_session_id = state
        .installation
        .authorize_oidc(&session_token, &csrf_token)
        .await?;
    let authorization_url = state
        .auth
        .begin_setup_login(
            &setup_session_id,
            &request.redirect_uri,
            &request.state,
            &request.code_challenge,
            &request.code_challenge_method,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(SetupOidcAuthorization { authorization_url }),
    ))
}

/// Extract the setup cookie and CSRF proof required by state-changing setup endpoints.
///
/// # Errors
/// Returns the mapped HTTP failure for invalid preconditions or a rejected resource operation;
/// internal diagnostics are not exposed in the response.
fn setup_credentials(
    installation: &InstallationService,
    headers: &HeaderMap,
) -> Result<(String, String), HttpError> {
    let session_token = setup_session_token(headers, installation.cookie_name())
        .ok_or(InstallationError::InvalidSession)?;
    let csrf_token = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or(InstallationError::CsrfMismatch)?
        .to_owned();
    Ok((session_token, csrf_token))
}

/// Read the setup credential from the deployment-appropriate cookie name.
fn setup_session_token(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    cookie_value(headers, cookie_name)
}
