//! Project deletion with published review history through both HTTP entry points.

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    revision: &str,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header(
                    "if-match",
                    if revision.parse::<i64>().is_ok() {
                        revision.to_owned()
                    } else {
                        format!("\"{revision}\"")
                    },
                )
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

async fn post(app: &Router, path: &str, body: Value, revision: &str) -> Value {
    let (status, value) = request(app, "POST", path, body, revision).await;
    assert!(status.is_success(), "{path}: {status}: {value}");
    value
}

#[tokio::test]
async fn deleting_projects_with_history_preserves_published_org_memory() {
    let pg = common::migrated_postgres().await;
    common::initialize_installation(
        pg.pool.clone(),
        "Deletion",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Surviving project",
    )
    .await;
    let (app, _) = common::authenticated_router(pg.pool.clone()).await;
    for prefix in ["/api/v1/admin/projects", "/api/v1/projects"] {
        let project = post(&app, prefix, json!({"name":prefix}), "1").await;
        let id = project["project_id"].as_str().unwrap();
        let (_, head) = request(&app, "GET", "/api/v1/org/commit-state", Value::Null, "1").await;
        let base = head["ref"]["commit_id"].as_str();
        let draft = post(&app, "/api/v1/drafts", json!({
            "daemon_installation_id":"deletion-test", "project_id":id,
            "base_commit_id":base, "title":"Published memory",
            "resource":{"scope":"org","path":format!("{id}.md")},
            "operations":[{"action":"create","resource":{"scope":"org","path":format!("{id}.md")},
                "content":{"content":"# Keep this published memory\n"}}]
        }), base.unwrap_or("ref-none")).await;
        let review = post(&app, "/api/v1/reviews", json!({"drafts":[{
            "draft_id":draft["draft"]["draft_id"], "expected_draft_version":draft["draft"]["version"]
        }]}), base.unwrap_or("ref-none")).await;
        post(
            &app,
            &format!(
                "/api/v1/reviews/{}/merges",
                review["review"]["review_id"].as_str().unwrap()
            ),
            json!({"expected_review_version":review["review"]["version"]}),
            base.unwrap_or("ref-none"),
        )
        .await;

        // Historical projects can also carry local commit chains, rebases, and assigned issues.
        // Seed those retired write paths without bypassing the public deletion operation.
        for statement in [
            "INSERT INTO trees (tree_id) VALUES ($1)",
            "INSERT INTO commits (commit_id, scope, org_id, project_id, tree_id, version)
             SELECT $1 || '-base', 'project', org_id, project_id, $1, 1 FROM projects WHERE project_id = $1",
            "INSERT INTO commits (commit_id, scope, org_id, project_id, tree_id, parent_commit_id, version)
             SELECT $1 || '-head', 'project', org_id, project_id, $1, $1 || '-base', 2 FROM projects WHERE project_id = $1",
            "UPDATE refs SET commit_id = $1 || '-head' WHERE project_id = $1",
            "INSERT INTO draft_revisions (revision_id, draft_id, draft_version, base_commit_id, lifecycle_status, title, description, operations)
             SELECT $1, draft_id, version, $1 || '-base', status, title, description, '[]'::jsonb FROM drafts WHERE project_id = $1",
            "INSERT INTO draft_reconciliation_candidates (candidate_id, draft_id, draft_version, base_commit_id, current_commit_id,
                status, base_state, current_state, draft_state, proposed_state, conflicts)
             SELECT $1, draft_id, version, $1 || '-base', $1 || '-head', 'clean', '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb
             FROM drafts WHERE project_id = $1",
            "INSERT INTO draft_rebases (rebase_id, draft_id, candidate_id, previous_revision_id, applied_by_user_id, resulting_draft_version, result_hash)
             SELECT $1, draft_id, $1, $1, author_user_id, version, 'hash' FROM drafts WHERE project_id = $1",
            "INSERT INTO kanban_issues (project_id, issue_id, issue_number, assignee_user_id, content_revision, payload)
             SELECT project_id, $1, 1, user_id, 1, '{}'::jsonb FROM project_members WHERE project_id = $1 LIMIT 1",
        ] {
            sqlx::query(statement).bind(id).execute(&pg.pool).await.unwrap();
        }

        let path = format!("{prefix}/{id}");
        let (status, _) = request(&app, "DELETE", &path, Value::Null, "999").await;
        assert_eq!(status, StatusCode::CONFLICT);
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM drafts WHERE project_id = $1")
            .bind(id)
            .fetch_one(&pg.pool)
            .await
            .unwrap();
        assert!(before > 0);
        let (status, result) = request(
            &app,
            "DELETE",
            &path,
            Value::Null,
            &project["revision"].to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["deleted"], true);
        for table in [
            "projects",
            "drafts",
            "reviews",
            "project_members",
            "refs",
            "commits",
            "kanban_issues",
        ] {
            let count: i64 = sqlx::query_scalar(&format!(
                "SELECT count(*) FROM {table} WHERE project_id = $1"
            ))
            .bind(id)
            .fetch_one(&pg.pool)
            .await
            .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        let published: i64 =
            sqlx::query_scalar("SELECT count(*) FROM resources WHERE scope = 'org' AND path = $1")
                .bind(format!("{id}.md"))
                .fetch_one(&pg.pool)
                .await
                .unwrap();
        assert_eq!(published, 1);
        let (status, _) = request(&app, "GET", "/api/v1/org/commit-state", Value::Null, "1").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = request(
            &app,
            "DELETE",
            &path,
            Value::Null,
            &project["revision"].to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    pg.shutdown().await;
}
