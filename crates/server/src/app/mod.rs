//! Resource modules and side-effect-free construction of the production router.

pub mod audit_event;
pub mod auth;
pub mod bundle;
pub mod commit;
pub mod draft;
pub mod health;
pub mod installation;
pub mod memory;
pub mod organization;
pub mod project;
pub mod review;
pub mod token;

use crate::app::auth::AuthService;
use crate::app::installation::InstallationService;
use crate::{routes, telemetry};
use axum::Router;
use sqlx::PgPool;

/// Build the production router from already-initialized dependencies.
///
/// Construction performs no I/O and starts no background tasks. The caller owns
/// database migration, listener binding, and graceful shutdown.
pub fn build_app(pool: PgPool, auth: AuthService, installation: InstallationService) -> Router {
    telemetry::instrument(routes::router_with_services(pool, auth, installation))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn every_response_has_one_request_id() {
        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(100))
            .connect_lazy("postgres://clumsies:clumsies@127.0.0.1:1/clumsies")
            .unwrap();
        let auth = AuthService::unconfigured(pool.clone());
        let installation = InstallationService::new(pool.clone(), None, true).unwrap();
        let app = build_app(pool, auth, installation);

        let success = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/health")
                    .header("x-request-id", "proxy-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(success.status(), StatusCode::OK);
        assert_eq!(success.headers()["x-request-id"], "proxy-123");

        let error = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(error.status(), StatusCode::UNAUTHORIZED);
        let response_request_id = error.headers()["x-request-id"].to_str().unwrap().to_owned();
        assert!(response_request_id.starts_with("req_"));
        let body: Value =
            serde_json::from_slice(&to_bytes(error.into_body(), 4 * 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["error"]["request_id"], response_request_id);
    }
}
