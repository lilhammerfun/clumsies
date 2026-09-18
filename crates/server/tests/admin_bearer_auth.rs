mod common;

use axum::body::Body;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn admin_routes_accept_only_an_organization_admin_bearer() {
    let postgres = common::migrated_postgres().await;
    common::initialize_installation(
        postgres.pool.clone(),
        "Clumsies Lab",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Default",
    )
    .await;
    let (_, token) = common::authenticated_router(postgres.pool.clone()).await;
    let app = common::router(postgres.pool.clone());

    let cookie_only = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/admin/org")
                .header(
                    COOKIE,
                    format!("clumsies_admin_session={}", token.access_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_only.status(), StatusCode::UNAUTHORIZED);

    let bearer = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/admin/org")
                .header(AUTHORIZATION, format!("Bearer {}", token.access_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bearer.status(), StatusCode::OK);

    let removed_session_route = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/admin/session")
                .header(AUTHORIZATION, format!("Bearer {}", token.access_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed_session_route.status(), StatusCode::NOT_FOUND);
    postgres.shutdown().await;
}
