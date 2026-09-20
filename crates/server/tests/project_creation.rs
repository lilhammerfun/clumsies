//! Idempotent project creation and replay through the production HTTP contract.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::LOCATION;
use axum::http::{Request, StatusCode};
use serde::Serialize;
use server::app::auth::AuthPrincipal;
use server::app::project::dto::{CreateProjectRequest, Project};
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn public_project_creation_is_atomic_and_idempotent() {
    let postgres = common::migrated_postgres().await;
    let installation = common::initialize_installation(
        postgres.pool.clone(),
        "Clumsies Lab",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Default",
    )
    .await;
    let (app, _) = common::authenticated_router(postgres.pool.clone()).await;
    let request = CreateProjectRequest {
        name: "  Product  ".to_owned(),
        description: Some("  Shared product memory  ".to_owned()),
    };

    let first = create_project(app.clone(), "project-create-1", &request).await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let location = first
        .headers()
        .get(LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let project: Project = decode_json(first).await;
    assert_eq!(project.name, "Product");
    assert_eq!(project.description, "Shared product memory");
    assert_eq!(location, format!("/api/v1/projects/{}", project.project_id));

    let replay = create_project(app.clone(), "project-create-1", &request).await;
    assert_eq!(replay.status(), StatusCode::CREATED);
    let replayed: Project = decode_json(replay).await;
    assert_eq!(replayed.project_id, project.project_id);

    let changed_request = create_project(
        app.clone(),
        "project-create-1",
        &CreateProjectRequest {
            name: "Different".to_owned(),
            description: None,
        },
    )
    .await;
    assert_eq!(changed_request.status(), StatusCode::CONFLICT);

    let duplicate_name = create_project(
        app.clone(),
        "project-create-2",
        &CreateProjectRequest {
            name: "product".to_owned(),
            description: None,
        },
    )
    .await;
    assert_eq!(duplicate_name.status(), StatusCode::CONFLICT);

    let missing_key = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);

    let member_role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(&project.project_id)
    .bind(&installation.user_id)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(member_role, "admin");

    let project_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM projects WHERE lower(name) = 'product'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(project_count, 1);
    postgres.shutdown().await;
}

#[tokio::test]
async fn failed_creator_membership_rolls_back_the_entire_project() {
    let postgres = common::migrated_postgres().await;
    let installation = common::initialize_installation(
        postgres.pool.clone(),
        "Clumsies Lab",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Default",
    )
    .await;
    let pool = postgres.pool.clone();
    let before = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM projects")
        .fetch_one(&postgres.pool)
        .await
        .unwrap();

    let result = server::app::project::create_project_from_request(
        &pool,
        &AuthPrincipal {
            user_id: "missing-user".to_owned(),
            org_id: installation.org_id,
            session_id: "session".to_owned(),
            token_id: "token".to_owned(),
            role: "admin".to_owned(),
        },
        CreateProjectRequest {
            name: "Must Roll Back".to_owned(),
            description: None,
        },
        "project-create-rollback",
    )
    .await;
    assert!(result.is_err());

    let after = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM projects")
        .fetch_one(&postgres.pool)
        .await
        .unwrap();
    assert_eq!(after, before);
    let request_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM project_creation_requests")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(request_count, 0);
    postgres.shutdown().await;
}

async fn create_project<T: Serialize>(
    app: Router,
    idempotency_key: &str,
    request: &T,
) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/projects")
            .header("content-type", "application/json")
            .header("idempotency-key", idempotency_key)
            .body(Body::from(serde_json::to_vec(request).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn decode_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
