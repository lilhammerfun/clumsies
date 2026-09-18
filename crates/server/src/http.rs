//! HTTP preconditions and error responses.

use crate::app::auth::{AuthError, AuthPrincipal};
use crate::app::installation::InstallationError;
use crate::error::ServerError;
use axum::Json;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub(crate) use crate::middleware::cookie_value;

pub(crate) fn require_org_admin(principal: &AuthPrincipal) -> Result<(), HttpError> {
    if principal.role == "owner" || principal.role == "admin" {
        Ok(())
    } else {
        Err(ServerError::Forbidden("organization administrator role required".to_owned()).into())
    }
}

pub(crate) fn parse_if_match(headers: &HeaderMap) -> Result<i64, HttpError> {
    let value = headers
        .get("if-match")
        .ok_or_else(|| HttpError::bad_request("missing If-Match header"))?
        .to_str()
        .map_err(|_| HttpError::bad_request("If-Match must be valid UTF-8"))?;
    let value = value.trim().trim_matches('"');
    value
        .parse::<i64>()
        .map_err(|_| HttpError::bad_request("If-Match must be an integer version"))
}

pub(crate) fn parse_idempotency_key(headers: &HeaderMap) -> Result<&str, HttpError> {
    let value = headers
        .get("idempotency-key")
        .ok_or_else(|| HttpError::bad_request("missing Idempotency-Key header"))?
        .to_str()
        .map_err(|_| HttpError::bad_request("Idempotency-Key must contain visible ASCII"))?;
    let value = value.trim();
    if value.is_empty() || value.len() > 200 {
        return Err(HttpError::bad_request(
            "Idempotency-Key must contain between 1 and 200 bytes",
        ));
    }
    Ok(value)
}

pub(crate) fn parse_ref_if_match(headers: &HeaderMap) -> Result<Option<String>, HttpError> {
    let value = headers
        .get("if-match")
        .ok_or_else(|| HttpError::bad_request("missing If-Match header"))?
        .to_str()
        .map_err(|_| HttpError::bad_request("If-Match must be valid UTF-8"))?
        .trim();
    if value.starts_with("W/") {
        return Err(HttpError::bad_request("ref If-Match must be a strong ETag"));
    }
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err(HttpError::bad_request("ref If-Match must be a quoted ETag"));
    }
    let opaque_tag = &value[1..value.len() - 1];
    match opaque_tag {
        "ref-none" => Ok(None),
        value if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            Ok(Some(value.to_owned()))
        }
        _ => Err(HttpError::bad_request(
            "ref If-Match contains an invalid commit ID",
        )),
    }
}

pub(crate) enum HttpError {
    Server(ServerError),
    Auth(AuthError),
    Installation(InstallationError),
    Internal(String),
}

impl HttpError {
    pub(crate) fn bad_request(message: &str) -> Self {
        Self::Server(ServerError::InvalidRequest(message.to_owned()))
    }

    pub(crate) fn internal(message: &str) -> Self {
        Self::Internal(message.to_owned())
    }
}

impl From<ServerError> for HttpError {
    fn from(error: ServerError) -> Self {
        Self::Server(error)
    }
}

impl From<AuthError> for HttpError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<InstallationError> for HttpError {
    fn from(error: InstallationError) -> Self {
        Self::Installation(error)
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let (status, code, message, details) = match self {
            Self::Server(error) => {
                let status = match &error {
                    ServerError::Forbidden(_) => StatusCode::FORBIDDEN,
                    ServerError::NotFound { .. } => StatusCode::NOT_FOUND,
                    ServerError::AlreadyExists { .. }
                    | ServerError::VersionConflict { .. }
                    | ServerError::ReconciliationRequired { .. }
                    | ServerError::DraftAlreadyCurrent { .. }
                    | ServerError::ReconciliationCandidateInvalid { .. } => StatusCode::CONFLICT,
                    ServerError::PreconditionFailed { .. } => StatusCode::PRECONDITION_FAILED,
                    ServerError::InvalidTransition { .. } | ServerError::InvalidRequest(_) => {
                        StatusCode::BAD_REQUEST
                    }
                    ServerError::Sqlx(_) => StatusCode::INTERNAL_SERVER_ERROR,
                };
                let code = match &error {
                    ServerError::Forbidden(_) => "forbidden",
                    ServerError::NotFound { .. } => "not_found",
                    ServerError::AlreadyExists { .. } => "already_exists",
                    ServerError::VersionConflict { .. } => "version_conflict",
                    ServerError::PreconditionFailed { .. } => "precondition_failed",
                    ServerError::ReconciliationRequired { .. } => "reconciliation_required",
                    ServerError::DraftAlreadyCurrent { .. } => "draft_already_current",
                    ServerError::ReconciliationCandidateInvalid { .. } => "candidate_invalid",
                    ServerError::InvalidTransition { .. } | ServerError::InvalidRequest(_) => {
                        "invalid_request"
                    }
                    ServerError::Sqlx(_) => "internal_error",
                };
                let details = match &error {
                    ServerError::VersionConflict {
                        entity,
                        expected,
                        actual,
                    } => json!({
                        "entity": entity,
                        "expected_version": expected,
                        "actual_version": actual,
                    }),
                    ServerError::PreconditionFailed { expected, actual } => json!({
                        "expected_commit_id": expected,
                        "current_commit_id": actual,
                    }),
                    ServerError::ReconciliationRequired {
                        draft_id,
                        candidate_id,
                        current_commit_id,
                    } => json!({
                        "draft_id": draft_id,
                        "candidate_id": candidate_id,
                        "current_commit_id": current_commit_id,
                    }),
                    ServerError::DraftAlreadyCurrent { draft_id } => json!({
                        "draft_id": draft_id,
                    }),
                    ServerError::ReconciliationCandidateInvalid { candidate_id } => json!({
                        "candidate_id": candidate_id,
                    }),
                    _ => json!({}),
                };
                (status, code, error.to_string(), details)
            }
            Self::Auth(error) => {
                let status = match &error {
                    AuthError::Unauthorized => StatusCode::UNAUTHORIZED,
                    AuthError::MemberNotAllowed
                    | AuthError::DomainNotAllowed
                    | AuthError::ProviderIdentityConflict => StatusCode::FORBIDDEN,
                    AuthError::NotConfigured | AuthError::ProviderUnavailable(_) => {
                        StatusCode::SERVICE_UNAVAILABLE
                    }
                    AuthError::Configuration(_) | AuthError::Sqlx(_) => {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                    AuthError::Installation(error) => installation_error_status(error),
                    _ => StatusCode::BAD_REQUEST,
                };
                (status, error.code(), error.to_string(), json!({}))
            }
            Self::Installation(error) => (
                installation_error_status(&error),
                error.code(),
                error.to_string(),
                json!({}),
            ),
            Self::Internal(message) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                message,
                json!({}),
            ),
        };
        let request_id = crate::telemetry::current_request_id();
        let mut response = (
            status,
            Json(json!({
                "error": {
                    "code": code,
                    "message": message,
                    "request_id": request_id,
                    "details": details
                }
            })),
        )
            .into_response();
        if let Ok(value) = HeaderValue::from_str(&request_id) {
            response
                .headers_mut()
                .insert(HeaderName::from_static("x-request-id"), value);
        }
        response
    }
}

fn installation_error_status(error: &InstallationError) -> StatusCode {
    match error {
        InstallationError::SetupRequired | InstallationError::Locked => StatusCode::CONFLICT,
        InstallationError::SetupUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        InstallationError::InvalidSetupCode | InstallationError::InvalidSession => {
            StatusCode::UNAUTHORIZED
        }
        InstallationError::CsrfMismatch | InstallationError::OwnerDomainNotAllowed => {
            StatusCode::FORBIDDEN
        }
        InstallationError::ConfigurationRequired
        | InstallationError::InvalidOwnerIdentity
        | InstallationError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
        InstallationError::Configuration(_)
        | InstallationError::CorruptSession
        | InstallationError::CorruptInstallation
        | InstallationError::Sqlx(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
