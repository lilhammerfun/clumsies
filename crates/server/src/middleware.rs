//! Authentication and security middleware.

use crate::app::auth::AuthError;
use crate::http::HttpError;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use cookie::Cookie;

/// Apply the shared browser-security response headers after executing the request.
pub(crate) async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'self'; base-uri 'none'; frame-ancestors 'none'; object-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data: https:; connect-src 'self'",
        ),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ] {
        response.headers_mut().insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    response
}

/// Resolve a bearer credential and attach its trusted principal to the request.
///
/// # Errors
/// Rejects missing or invalid bearer credentials, incomplete installation, and authentication
/// persistence failures.
pub(crate) async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, HttpError> {
    let bearer_token = bearer_token(request.headers()).ok_or(AuthError::Unauthorized)?;
    let principal = state.auth.authenticate(bearer_token).await?;
    request.extensions_mut().insert(principal);
    Ok(next.run(request).await)
}

/// Authenticate the request and reject identities without organization administration privileges.
///
/// # Errors
/// Rejects invalid authentication and principals without organization-administration privileges.
pub(crate) async fn require_admin_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, HttpError> {
    let bearer_token = bearer_token(request.headers()).ok_or(AuthError::Unauthorized)?;
    let principal = state.auth.authenticate(bearer_token).await?;
    principal.require_org_admin()?;
    request.extensions_mut().insert(principal);
    Ok(next.run(request).await)
}

/// Extract a well-formed bearer credential from the Authorization header.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
}

/// Find one named cookie without interpreting unrelated cookie values.
pub(crate) fn cookie_value(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(Cookie::split_parse)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == cookie_name)
        .map(|cookie| cookie.value().to_owned())
}
