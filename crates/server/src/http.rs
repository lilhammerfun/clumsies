use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use serde_json::json;
use sqlx::PgPool;

use crate::auth::{AuthError, AuthPrincipal, AuthService};
pub use crate::health::{AdminHealth, HealthCheck, HealthStatus};
use crate::installation::{InstallationError, InstallationService};
pub(crate) use crate::middleware::cookie_value;
use crate::middleware::{require_admin_auth, require_auth, security_headers};
use crate::repository::{ServerError, ServerRepository};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) pool: PgPool,
    pub(crate) repository: ServerRepository,
    pub(crate) auth: AuthService,
    pub(crate) installation: InstallationService,
    pub(crate) version: &'static str,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct HttpOperation {
    method: &'static str,
    path: &'static str,
}

macro_rules! define_routes {
    ($function:ident, $operations:ident, {
        $(
            $path:literal => {
                $first_method:ident: $first_handler:path
                $(, $method:ident: $handler:path)*
                $(,)?
            };
        )*
    }) => {
        #[cfg(test)]
        const $operations: &[HttpOperation] = &[
            $(
                HttpOperation {
                    method: stringify!($first_method),
                    path: $path,
                },
                $(
                    HttpOperation {
                        method: stringify!($method),
                        path: $path,
                    },
                )*
            )*
        ];

        fn $function() -> Router<AppState> {
            Router::new()
                $(.route($path, $first_method($first_handler)$(.$method($handler))*))*
        }
    };
}

define_routes!(public_routes, PUBLIC_OPERATIONS, {
    "/api/v1/admin/health" => { get: crate::health::admin_health };
    "/api/v1/setup" => { get: crate::installation::http::get_setup };
    "/api/v1/setup/sessions" => { post: crate::installation::http::create_setup_session };
    "/api/v1/setup/configuration" => { put: crate::installation::http::replace_setup_configuration };
    "/api/v1/setup/oidc-authorizations" => { post: crate::installation::http::create_setup_oidc_authorization };
    "/oauth2/authorization/oidc" => { get: crate::auth::http::begin_oidc };
    "/login/oauth2/code/oidc" => { get: crate::auth::http::complete_oidc };
    "/api/v1/auth/token" => { post: crate::auth::http::exchange_auth_token };
});

define_routes!(admin_routes, ADMIN_OPERATIONS, {
    "/api/v1/admin/identity-provider" => { get: crate::auth::http::get_admin_identity_provider };
    "/api/v1/admin/org" => { get: crate::organization::http::get_admin_org, patch: crate::organization::http::update_admin_org };
    "/api/v1/admin/members" => {
        get: crate::organization::http::list_admin_members,
        post: crate::organization::http::create_admin_member,
    };
    "/api/v1/admin/members/{user_id}" => {
        patch: crate::organization::http::update_admin_member,
        delete: crate::organization::http::delete_admin_member,
    };
    "/api/v1/admin/projects" => { get: crate::organization::http::list_admin_projects, post: crate::organization::http::create_admin_project };
    "/api/v1/admin/tokens" => { get: crate::organization::http::list_admin_tokens };
    "/api/v1/admin/tokens/{token_id}" => { delete: crate::organization::http::delete_admin_token };
    "/api/v1/admin/audit-events" => { get: crate::organization::http::list_admin_audit_events };
    "/api/v1/admin/memory-export" => { get: crate::memory::http::export_org_memory_state };
});

define_routes!(protected_routes, PROTECTED_OPERATIONS, {
    "/api/v1/auth/session" => { delete: crate::auth::http::revoke_auth_session };
    "/api/v1/me" => { get: crate::auth::http::get_me };
    "/api/v1/projects" => { get: crate::organization::http::list_projects, post: crate::organization::http::create_project };
    "/api/v1/projects/{project_id}" => {
        get: crate::organization::http::get_project,
        patch: crate::organization::http::update_project,
        delete: crate::organization::http::delete_project,
    };
    "/api/v1/admin/projects/{project_id}" => {
        get: crate::organization::http::get_admin_project,
        patch: crate::organization::http::update_admin_project,
        delete: crate::organization::http::delete_admin_project,
    };
    "/api/v1/admin/projects/{project_id}/member-candidates" => { get: crate::organization::http::list_project_member_candidates };
    "/api/v1/admin/projects/{project_id}/members" => {
        get: crate::organization::http::list_admin_project_members,
        post: crate::organization::http::create_admin_project_member,
    };
    "/api/v1/admin/projects/{project_id}/members/{user_id}" => {
        patch: crate::organization::http::update_admin_project_member,
        delete: crate::organization::http::delete_admin_project_member,
    };
    "/api/v1/projects/{project_id}/members" => { get: crate::organization::http::list_project_members };
    "/api/v1/me/bundles" => {
        get: crate::memory::http::list_personal_bundles,
        post: crate::memory::http::create_personal_bundle,
    };
    "/api/v1/me/bundles/{bundle_id}" => {
        get: crate::memory::http::get_personal_bundle,
        patch: crate::memory::http::update_personal_bundle,
        delete: crate::memory::http::delete_personal_bundle,
    };
    "/api/v1/org/memories" => { get: crate::memory::http::list_org_memories };
    "/api/v1/org/memories/{memory_id}" => { get: crate::memory::http::get_org_memory };
    "/api/v1/projects/{project_id}/memories" => { get: crate::memory::http::list_project_memories };
    "/api/v1/projects/{project_id}/memories/{memory_id}" => { get: crate::memory::http::get_project_memory };
    "/api/v1/projects/{project_id}/org-selections" => {
        get: crate::memory::http::get_project_org_selection,
        put: crate::memory::http::replace_project_org_selection,
    };
    "/api/v1/drafts" => { get: crate::changes::http::list_drafts, post: crate::changes::http::create_draft };
    "/api/v1/drafts/{draft_id}" => {
        get: crate::changes::http::get_draft,
        patch: crate::changes::http::update_draft,
        delete: crate::changes::http::delete_draft,
    };
    "/api/v1/drafts/{draft_id}/operations" => { post: crate::changes::http::append_draft_operation };
    "/api/v1/drafts/{draft_id}/reconciliation-candidates" => {
        post: crate::changes::http::create_draft_reconciliation_candidate,
    };
    "/api/v1/drafts/{draft_id}/reconciliation-candidates/{candidate_id}" => {
        get: crate::changes::http::get_draft_reconciliation_candidate,
    };
    "/api/v1/drafts/{draft_id}/rebases" => { post: crate::changes::http::create_draft_rebase };
    "/api/v1/draft-events" => { get: crate::changes::http::list_draft_events };
    "/api/v1/draft-operation-batches" => { post: crate::changes::http::create_draft_operation_batch };
    "/api/v1/reviews" => { get: crate::changes::http::list_reviews, post: crate::changes::http::create_review };
    "/api/v1/reviews/{review_id}" => { get: crate::changes::http::get_review };
    "/api/v1/reviews/{review_id}/comments" => {
        get: crate::changes::http::list_review_comments,
        post: crate::changes::http::create_review_comment,
    };
    "/api/v1/reviews/{review_id}/decisions" => { post: crate::changes::http::create_review_decision };
    "/api/v1/reviews/{review_id}/submissions" => { post: crate::changes::http::create_review_submission };
    "/api/v1/reviews/{review_id}/merges" => { post: crate::changes::http::create_review_merge };
    "/api/v1/org/commits" => { get: crate::memory::http::list_org_commits };
    "/api/v1/org/commit-state" => { get: crate::memory::http::get_org_commit_state };
    "/api/v1/projects/{project_id}/commits" => { get: crate::memory::http::list_project_commits };
    "/api/v1/projects/{project_id}/commit-state" => { get: crate::memory::http::get_project_commit_state };
    "/api/v1/commits/{commit_id}" => { get: crate::memory::http::get_commit };
});

pub fn router(pool: PgPool) -> Router {
    let auth = AuthService::unconfigured(pool.clone());
    let installation = InstallationService::new(pool.clone(), None, true)
        .expect("an installation service without a setup code is valid");
    router_with_services(pool, auth, installation)
}

pub fn router_with_auth(pool: PgPool, auth: AuthService) -> Router {
    let installation = InstallationService::new(pool.clone(), None, true)
        .expect("an installation service without a setup code is valid");
    router_with_services(pool, auth, installation)
}

pub fn router_with_services(
    pool: PgPool,
    auth: AuthService,
    installation: InstallationService,
) -> Router {
    let state = AppState {
        repository: ServerRepository::new(pool.clone()),
        auth,
        installation,
        pool,
        version: env!("CARGO_PKG_VERSION"),
    };
    let public_routes = public_routes();
    let admin_routes = admin_routes().route_layer(middleware::from_fn_with_state(
        state.clone(),
        require_admin_auth,
    ));
    let protected_routes =
        protected_routes().route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    Router::new()
        .merge(public_routes)
        .merge(admin_routes)
        .merge(protected_routes)
        .with_state(state)
        .layer(middleware::from_fn(security_headers))
}

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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde::Deserialize;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use crate::http::{
        ADMIN_OPERATIONS, AdminHealth, HealthStatus, PROTECTED_OPERATIONS, PUBLIC_OPERATIONS,
        router,
    };

    #[derive(Debug, Deserialize)]
    struct OpenApiDocument {
        paths: BTreeMap<String, BTreeMap<String, serde_yaml_ng::Value>>,
    }

    #[test]
    fn axum_routes_match_public_and_admin_openapi() {
        let public = include_str!("../openapi/clumsies.public.v1.yaml");
        let admin = include_str!("../openapi/clumsies.admin.v1.yaml");
        let contract_operations = openapi_operations(public)
            .into_iter()
            .chain(openapi_operations(admin))
            .collect::<BTreeSet<_>>();
        let server_operations = PUBLIC_OPERATIONS
            .iter()
            .chain(ADMIN_OPERATIONS)
            .chain(PROTECTED_OPERATIONS)
            .map(|operation| (operation.method.to_owned(), operation.path.to_owned()))
            .collect::<BTreeSet<_>>();

        assert_eq!(server_operations, contract_operations);
    }

    #[tokio::test]
    async fn admin_health_matches_contract_shape_when_database_is_down() {
        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(100))
            .connect_lazy("postgres://clumsies:clumsies@127.0.0.1:1/clumsies")
            .unwrap();
        let app = router(pool);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let health: AdminHealth = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, HealthStatus::Down);
        assert_eq!(health.database.status, HealthStatus::Down);
        assert_eq!(health.schema.status, HealthStatus::Down);
        assert_eq!(health.commit_service.status, HealthStatus::Down);
        assert_eq!(health.oidc.status, HealthStatus::Down);
    }

    fn openapi_operations(source: &str) -> BTreeSet<(String, String)> {
        const HTTP_METHODS: [&str; 8] = [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ];
        let document: OpenApiDocument = serde_yaml_ng::from_str(source).unwrap();
        document
            .paths
            .into_iter()
            .flat_map(|(path, item)| {
                item.into_keys()
                    .filter(|method| HTTP_METHODS.contains(&method.as_str()))
                    .map(move |method| (method, path.clone()))
            })
            .collect()
    }
}
