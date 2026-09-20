//! Published Dashboard contract, complete inventory and authorization regression.
mod common;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use server::app::{
    auth::AuthPrincipal,
    memory::{
        self,
        dto::{MemoryListResponse, MemoryStatistics},
    },
};
use tower::ServiceExt;

#[tokio::test]
async fn dashboard_counts_full_history_and_respects_project_access() {
    let db = common::migrated_postgres().await;
    let setup = common::initialize_installation(
        db.pool.clone(),
        "Dashboard",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Dashboard",
    )
    .await;
    let principal = AuthPrincipal {
        user_id: setup.user_id.clone(),
        org_id: setup.org_id.clone(),
        session_id: "test-session".into(),
        token_id: "test-token".into(),
        role: "owner".into(),
    };
    let mut ids = Vec::new();
    for i in 0..244 {
        ids.push(
            memory::create_org_context(
                &db.pool,
                &principal,
                &format!("knowledge/{i}.md"),
                "# Memory",
            )
            .await
            .unwrap(),
        );
    }
    sqlx::query("UPDATE commits SET created_at = now() - interval '100 days' WHERE scope = 'org' AND version <= 200").execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE commits SET created_at = now() - interval '2 days' WHERE scope = 'org' AND version > 200").execute(&db.pool).await.unwrap();
    let (_, token) = common::authenticated_router(db.pool.clone()).await;
    let app = common::router(db.pool.clone());
    let get = |path: String| {
        Request::builder()
            .uri(path)
            .header("authorization", format!("Bearer {}", token.access_token))
            .body(Body::empty())
            .unwrap()
    };
    let response = app
        .clone()
        .oneshot(get("/api/v1/org/memories".into()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let list: MemoryListResponse = serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        list.items.len(),
        244,
        "Metadata must not silently stop at 200"
    );
    for revision in 1..=2 {
        sqlx::query("UPDATE resources SET path = $1 WHERE resource_id = $2")
            .bind(format!("renamed/{revision}.md"))
            .bind(&ids[0])
            .execute(&db.pool)
            .await
            .unwrap();
        memory::create_org_context(&db.pool, &principal, &format!("new/{revision}.md"), "# New")
            .await
            .unwrap();
    }
    sqlx::query("UPDATE resources SET status = 'archived' WHERE resource_id = $1")
        .bind(&ids[1])
        .execute(&db.pool)
        .await
        .unwrap();
    memory::create_org_context(&db.pool, &principal, "new/3.md", "# New")
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(get(
            "/api/v1/org/memory-statistics?days=30&time_zone=Asia%2FShanghai".into(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let stats: MemoryStatistics = serde_json::from_slice(&body).unwrap();
    assert_eq!(stats.memory_count, 246);
    assert_eq!(stats.resources.len(), 246);
    assert_eq!(stats.added_count, 47);
    assert_eq!(
        stats.updated_count, 1,
        "Repeated edits count once in the period"
    );
    assert_eq!(stats.deleted_count, 1);
    assert_eq!(stats.days.len(), 30);
    assert_eq!(stats.days.first().unwrap().memory_count, Some(200));
    assert_eq!(stats.days.last().unwrap().memory_count, Some(246));
    assert_eq!(stats.day_bounds.len(), 31);
    assert!(
        stats
            .day_bounds
            .iter()
            .all(|bound| (bound + 8 * 3600) % 86400 == 0)
    );
    assert_eq!(stats.project_ids, vec![setup.project_id.clone()]);
    memory::select_org_resource_for_project(&db.pool, &principal, &setup.project_id, &ids[0])
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(get(format!(
            "/api/v1/projects/{}/memory-statistics?days=7&time_zone=UTC",
            setup.project_id
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let project: MemoryStatistics =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(project.memory_count, 1);
    assert_eq!(project.resources[0].id, ids[0]);
    for path in [
        "/api/v1/org/memory-statistics?days=8&time_zone=UTC",
        "/api/v1/org/memory-statistics?days=7&time_zone=invalid",
    ] {
        assert_eq!(
            app.clone()
                .oneshot(get(path.into()))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let other = app
        .clone()
        .oneshot(get(
            "/api/v1/projects/inaccessible/memory-statistics?days=7&time_zone=UTC".into(),
        ))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::NOT_FOUND);
    let unauth = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/org/memory-statistics?days=7&time_zone=UTC")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
}
