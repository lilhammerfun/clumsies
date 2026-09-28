//! Production readiness responses with available and unavailable dependencies.

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use common::router;
use server::app::health::{AdminHealth, HealthStatus};
use server::infra::database::current_schema_migration;
use tower::ServiceExt;

#[tokio::test]
async fn health_after_migrations_reports_database_and_schema_ready() {
    let postgres = common::migrated_postgres().await;
    let app = router(postgres.pool.clone());
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
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let health: AdminHealth = serde_json::from_slice(&body).unwrap();
    assert_eq!(health.status, HealthStatus::Ok);
    assert_eq!(health.database.status, HealthStatus::Ok);
    assert_eq!(health.schema.status, HealthStatus::Ok);
    assert_eq!(
        health.schema.message,
        format!("migration {} applied", current_schema_migration())
    );
    assert_eq!(health.commit_service.status, HealthStatus::Ok);
    assert_eq!(health.oidc.status, HealthStatus::Ok);
    postgres.shutdown().await;
}

#[tokio::test]
async fn metrics_endpoint_keeps_scrape_contract_without_a_database_connection() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(17)
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    let app = router(pool);
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/plain; version=0.0.4")
        );
        let text = String::from_utf8(
            to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(text.contains("# TYPE clumsies_db_pool_size gauge"));
        assert!(text.contains("clumsies_db_pool_size 0\n"));
        assert!(text.contains("clumsies_db_pool_max_connections 17\n"));
        assert!(text.contains("clumsies_build_info{version="));
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let text = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(text.contains("clumsies_http_requests_total{route=\"/metrics\",status=\"200\"} 2"));
    assert!(text.contains("clumsies_http_request_duration_seconds_count{route=\"/metrics\"} 2"));
}
