//! Internal values and invariants for token resources.

use crate::app::token::dto::AccessTokenKind;
use crate::error::ServerError;

/// Decode a persisted credential category without inspecting credential secrets.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
pub(crate) fn access_token_kind(value: &str) -> Result<AccessTokenKind, ServerError> {
    match value {
        "access" => Ok(AccessTokenKind::Access),
        "refresh" => Ok(AccessTokenKind::Refresh),
        "integration" => Ok(AccessTokenKind::Integration),
        "web_session" => Ok(AccessTokenKind::WebSession),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown access token kind: {other}"
        ))),
    }
}
