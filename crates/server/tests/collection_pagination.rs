//! Public Project and Review pagination through authenticated HTTP and real PostgreSQL.

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tower::ServiceExt;

mod common;

async fn request(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("if-match", "\"ref-none\"")
                .header("idempotency-key", uuid::Uuid::new_v4().to_string())
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn post(app: &Router, path: &str, body: Value) -> Value {
    let (status, value) = request(app, "POST", path, body).await;
    assert!(status.is_success(), "{path}: {status}: {value}");
    value
}

async fn pages(app: &Router, path: &str, id: &str, expected: usize) {
    let mut cursor = None;
    let mut ids = BTreeSet::new();
    loop {
        let mut uri = format!(
            "{path}{}limit=1",
            if path.contains('?') { '&' } else { '?' }
        );
        if let Some(cursor) = &cursor {
            uri.push_str(&format!("&cursor={cursor}"));
        }
        let (status, page) = request(app, "GET", &uri, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let items = page["items"].as_array().unwrap();
        assert!(items.len() <= 1);
        for item in items {
            assert!(
                ids.insert(item[id].as_str().unwrap().to_owned()),
                "duplicate item: {page}"
            );
        }
        assert!(ids.len() <= expected, "pagination did not terminate");
        cursor = page["page_info"]["next_cursor"].as_str().map(str::to_owned);
        assert_eq!(page["page_info"]["has_more"], cursor.is_some());
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), expected);
}

#[tokio::test]
async fn collections_continue_with_tied_timestamps_and_validate_page_inputs() {
    let pg = common::migrated_postgres().await;
    common::initialize_installation(
        pg.pool.clone(),
        "Pagination",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Initial",
    )
    .await;
    let (app, _) = common::authenticated_router(pg.pool.clone()).await;
    let mut project = String::new();
    for n in 0..3 {
        let p = post(
            &app,
            "/api/v1/projects",
            json!({"name":format!("Project {n}")}),
        )
        .await;
        project = p["project_id"].as_str().unwrap().to_owned();
        let draft = post(&app, "/api/v1/drafts", json!({
            "daemon_installation_id":"pagination-test", "project_id":project,
            "base_commit_id":null, "title":"Pagination",
            "resource":{"scope":"org","path":format!("page-{n}.md")},
            "operations":[{"action":"create","resource":{"scope":"org","path":format!("page-{n}.md")},"content":{"content":"pagination"}}]
        })).await;
        post(&app, "/api/v1/reviews", json!({"drafts":[{"draft_id":draft["draft"]["draft_id"],"expected_draft_version":draft["draft"]["version"]}]})).await;
    }
    for table in ["projects", "reviews"] {
        sqlx::query(&format!(
            "UPDATE {table} SET updated_at = '2026-01-01T00:00:00Z'"
        ))
        .execute(&pg.pool)
        .await
        .unwrap();
    }
    pages(&app, "/api/v1/projects", "project_id", 4).await;
    pages(&app, "/api/v1/reviews", "review_id", 3).await;
    pages(
        &app,
        &format!("/api/v1/reviews?project_id={project}"),
        "review_id",
        1,
    )
    .await;
    for path in ["/api/v1/projects", "/api/v1/reviews"] {
        for query in ["limit=0", "limit=201", "cursor=invalid", "cursor=-1"] {
            let (status, _) = request(&app, "GET", &format!("{path}?{query}"), Value::Null).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
    }
    pg.shutdown().await;
}
