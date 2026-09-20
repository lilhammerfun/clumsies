//! Exercises notification delivery, receipt versions, and revoked membership over HTTP.
mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use server::app::inbox::dto::InboxListResponse;
use tower::ServiceExt;

async fn request(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn inbox(app: &Router) -> InboxListResponse {
    let (status, body) = request(app, "GET", "/api/v1/me/inbox", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn post_with_ref(app: &Router, path: &str, body: Value) -> (StatusCode, Value) {
    post_at_ref(app, path, body, "ref-none").await
}

async fn post_at_ref(app: &Router, path: &str, body: Value, commit: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .header("if-match", format!("\"{commit}\""))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn review_notifications_survive_refresh_and_old_receipts_do_not_hide_new_events() {
    let db = common::migrated_postgres().await;
    let installation = common::initialize_installation(
        db.pool.clone(),
        "Inbox",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Shared",
    )
    .await;
    sqlx::query(
        "INSERT INTO users (user_id, email, display_name, role, status)
        VALUES ('usr_author', 'author@example.com', 'Author', 'member', 'active'),
               ('usr_outsider', 'outsider@example.com', 'Outsider', 'admin', 'active')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, 'usr_author', 'member')")
        .bind(&installation.project_id).execute(&db.pool).await.unwrap();
    let (owner, _) = common::authenticated_router(db.pool.clone()).await;
    let (author, _) = common::authenticated_router_as(
        db.pool.clone(),
        "author@example.com",
        "subject-author",
        "Author",
    )
    .await;
    let (outsider, _) = common::authenticated_router_as(
        db.pool.clone(),
        "outsider@example.com",
        "subject-outsider",
        "Outsider",
    )
    .await;
    let (status, draft) = request(&author, "POST", "/api/v1/drafts", json!({
        "daemon_installation_id": "inbox-test", "project_id": installation.project_id,
        "base_commit_id": null, "title": "Add guide", "resource": {"scope": "org", "path": "guide.md"},
        "operations": [{"action": "create", "resource": {"scope": "org", "path": "guide.md"},
            "content": {"content": "# Guide"}}]
    })).await;
    assert_eq!(status, StatusCode::OK, "{draft}");
    let response = author
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/reviews")
                .header("content-type", "application/json")
                .header("if-match", "\"ref-none\"")
                .body(Body::from(
                    json!({"drafts": [{"draft_id": draft["draft"]["draft_id"],
            "expected_draft_version": draft["draft"]["version"]}], "title": "Add guide"})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let review: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(status, StatusCode::OK, "{review}");
    let review_id = review["review"]["review_id"].as_str().unwrap();
    let version = review["review"]["version"].as_i64().unwrap();
    let first = inbox(&owner).await;
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].kind, "review_requested");
    assert!(first.items[0].needs_action);
    assert!(inbox(&author).await.items.is_empty());
    assert!(
        inbox(&outsider).await.items.is_empty(),
        "An org admin outside the project is not a recipient."
    );
    let receipt_path = format!("/api/v1/me/inbox/{}", first.items[0].notification_id);
    assert_eq!(
        request(
            &outsider,
            "PATCH",
            &receipt_path,
            json!({"version": 1, "action": "read"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &owner,
            "PATCH",
            &receipt_path,
            json!({"version": 1, "action": "archive"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(inbox(&owner).await.items[0].archived_version, 1);
    assert_eq!(
        inbox(&owner).await.items[0].read_version,
        0,
        "Archive must preserve unread state."
    );
    for (action, expected_read, expected_archive) in [
        ("read", 1, 1),
        ("unread", 0, 1),
        ("restore", 0, 0),
        ("read", 1, 0),
        ("archive", 1, 1),
    ] {
        assert_eq!(
            request(
                &owner,
                "PATCH",
                &receipt_path,
                json!({"version": 1, "action": action})
            )
            .await
            .0,
            StatusCode::OK
        );
        let receipt = inbox(&owner).await;
        assert_eq!(receipt.items[0].read_version, expected_read);
        assert_eq!(receipt.items[0].archived_version, expected_archive);
    }

    let comments_path = format!("/api/v1/reviews/{review_id}/comments");
    assert_eq!(
        request(
            &owner,
            "POST",
            &comments_path,
            json!({"body": "Please clarify.", "expected_review_version": version})
        )
        .await
        .0,
        StatusCode::OK
    );
    let author_notice = inbox(&author).await;
    assert_eq!(author_notice.items[0].kind, "review_comment");
    assert_eq!(
        request(
            &author,
            "POST",
            &comments_path,
            json!({"body": "Clarified.", "expected_review_version": version})
        )
        .await
        .0,
        StatusCode::OK
    );
    let next = inbox(&owner).await;
    assert_eq!(
        next.items.len(),
        1,
        "Discussion updates aggregate under the review."
    );
    assert_eq!(next.items[0].version, 2);
    assert!(
        next.items[0].needs_action,
        "A reply must not conceal the outstanding review decision."
    );
    assert_eq!(next.items[0].archived_version, 1);
    assert_eq!(
        request(
            &owner,
            "PATCH",
            &receipt_path,
            json!({"version": 1, "action": "archive"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(inbox(&owner).await.items[0].read_version, 1);
    let count_before: i64 =
        sqlx::query_scalar("SELECT sum(version)::bigint FROM inbox_notifications")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        request(
            &owner,
            "POST",
            &comments_path,
            json!({"body": "Stale comment", "expected_review_version": version + 10})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let count_after: i64 =
        sqlx::query_scalar("SELECT sum(version)::bigint FROM inbox_notifications")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count_before, count_after);
    let (status, rejected_review) = request(
        &owner,
        "POST",
        &format!("/api/v1/reviews/{review_id}/decisions"),
        json!({"decision": "rejected", "expected_review_version": version, "body": "Needs work"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rejected = inbox(&author).await;
    assert_eq!(rejected.items[0].kind, "review_rejected");
    assert!(rejected.items[0].needs_action);
    assert!(!inbox(&owner).await.items[0].needs_action);
    let draft_path = format!(
        "/api/v1/drafts/{}",
        draft["draft"]["draft_id"].as_str().unwrap()
    );
    let (_, reopened) = request(&author, "GET", &draft_path, Value::Null).await;
    let (status, resubmitted) = post_with_ref(&author, &format!("/api/v1/reviews/{review_id}/submissions"), json!({
        "expected_review_version": rejected_review["review"]["version"],
        "drafts": [{"draft_id": reopened["draft"]["draft_id"], "expected_draft_version": reopened["draft"]["version"]}]
    })).await;
    assert_eq!(status, StatusCode::OK, "{resubmitted}");
    let request_again = inbox(&owner).await;
    assert_eq!(request_again.items[0].kind, "review_requested");
    assert_eq!(request_again.items[0].version, 3);
    assert!(request_again.items[0].needs_action);
    let merge_path = format!("/api/v1/reviews/{review_id}/merges");
    let merge_body = json!({"expected_review_version": resubmitted["review"]["version"]});
    let (status, merged) = post_with_ref(&owner, &merge_path, merge_body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{merged}");
    let published = inbox(&author).await;
    assert_eq!(published.items.len(), 2);
    let outcome = published
        .items
        .iter()
        .find(|item| item.kind == "review_merged")
        .unwrap();
    assert_eq!(
        outcome.version, 3,
        "Direct merge emits one outcome, without an intermediate approval."
    );
    assert!(!outcome.needs_action);
    let shared = published
        .items
        .iter()
        .find(|item| item.kind == "shared_update")
        .unwrap();
    assert_eq!(shared.project_id, installation.project_id);
    assert_eq!(shared.version, 1);
    assert!(inbox(&outsider).await.items.is_empty());
    assert!(!inbox(&owner).await.items[0].needs_action);
    let (status, first_page) =
        request(&author, "GET", "/api/v1/me/inbox?limit=1", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let first_page: InboxListResponse = serde_json::from_value(first_page).unwrap();
    assert_eq!(first_page.items.len(), 1);
    let (_, second_page) = request(
        &author,
        "GET",
        &format!(
            "/api/v1/me/inbox?limit=1&cursor={}",
            first_page.next_cursor.unwrap()
        ),
        Value::Null,
    )
    .await;
    let second_page: InboxListResponse = serde_json::from_value(second_page).unwrap();
    assert_eq!(second_page.items.len(), 1);
    assert!(second_page.next_cursor.is_none());
    assert_ne!(
        first_page.items[0].notification_id,
        second_page.items[0].notification_id
    );
    assert_ne!(
        post_with_ref(&owner, &merge_path, merge_body).await.0,
        StatusCode::OK
    );
    assert_eq!(
        inbox(&author)
            .await
            .items
            .iter()
            .map(|item| item.version)
            .collect::<Vec<_>>(),
        vec![3, 1]
    );
    assert_eq!(
        request(
            &owner,
            "PATCH",
            &receipt_path,
            json!({"version": 3, "action": "archive"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &owner,
            "PATCH",
            &receipt_path,
            json!({"version": 1, "action": "restore"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        inbox(&owner).await.items[0].archived_version,
        3,
        "Old restore cannot undo a newer archive."
    );
    assert_eq!(
        request(
            &owner,
            "PATCH",
            &receipt_path,
            json!({"version": 3, "action": "read"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &owner,
            "PATCH",
            &receipt_path,
            json!({"version": 1, "action": "unread"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        inbox(&owner).await.items[0].read_version,
        3,
        "An old unread action cannot undo a newer read receipt."
    );
    assert_eq!(
        request(
            &owner,
            "PATCH",
            &receipt_path,
            json!({"version": 3, "action": "unread"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(inbox(&owner).await.items[0].read_version, 0);
    assert_eq!(inbox(&owner).await.items[0].archived_version, 3);
    sqlx::query("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2")
        .bind(&installation.project_id)
        .bind(&installation.user_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(inbox(&owner).await.items.is_empty());
    assert_eq!(
        request(
            &owner,
            "PATCH",
            &receipt_path,
            json!({"version": 2, "action": "read"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    db.shutdown().await;
}

#[tokio::test]
async fn shared_updates_only_notify_members_of_projects_using_the_published_memory() {
    let db = common::migrated_postgres().await;
    let installation = common::initialize_installation(
        db.pool.clone(),
        "Inbox references",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Publisher",
    )
    .await;
    let principal = common::owner_principal(&db.pool).await;
    let resource_id =
        server::app::memory::create_org_context(&db.pool, &principal, "guide.md", "# Original")
            .await
            .unwrap();
    let unrelated_resource = server::app::memory::create_org_context(
        &db.pool,
        &principal,
        "unrelated.md",
        "# Unrelated",
    )
    .await
    .unwrap();
    let mut projects = Vec::new();
    for name in ["Uses guide", "Uses other memory"] {
        projects.push(
            server::app::project::create_project(&db.pool, &principal, name, "")
                .await
                .unwrap(),
        );
    }
    sqlx::query(
        "INSERT INTO users (user_id, email, display_name, role, status)
        VALUES ('usr_reader', 'reader@example.com', 'Reader', 'member', 'active')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    for project_id in &projects {
        sqlx::query("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, 'usr_reader', 'member')")
            .bind(project_id).execute(&db.pool).await.unwrap();
    }
    for (project_id, selected) in [
        (&installation.project_id, &resource_id),
        (&projects[0], &resource_id),
        (&projects[1], &unrelated_resource),
    ] {
        server::app::memory::select_org_resource_for_project(
            &db.pool, &principal, project_id, selected,
        )
        .await
        .unwrap();
    }
    let (owner, _) = common::authenticated_router(db.pool.clone()).await;
    let (reader, _) = common::authenticated_router_as(
        db.pool.clone(),
        "reader@example.com",
        "subject-reader",
        "Reader",
    )
    .await;
    assert!(
        inbox(&reader).await.items.is_empty(),
        "Selecting Memory is not a remote update."
    );
    for (index, action) in ["update", "rename", "delete"].into_iter().enumerate() {
        let (_, state) = request(&owner, "GET", "/api/v1/org/commit-state", Value::Null).await;
        let commit = state["ref"]["commit_id"].as_str().unwrap();
        let resource = json!({"scope": "org", "id": resource_id});
        let (status, draft) = request(&owner, "POST", "/api/v1/drafts", json!({
            "daemon_installation_id": "inbox-references", "project_id": installation.project_id,
            "base_commit_id": commit, "title": action, "resource": resource,
            "operations": [{"action": action, "resource": resource,
                "content": if action == "update" { json!({"content": "# Updated"}) } else { Value::Null },
                "new_path": if action == "rename" { json!("renamed.md") } else { Value::Null }}]
        })).await;
        assert_eq!(status, StatusCode::OK, "{draft}");
        let before = inbox(&reader).await;
        assert_eq!(
            before.items.first().map(|item| item.version).unwrap_or(0),
            index as i64,
            "Saving a Draft must not produce a shared update."
        );
        let (status, review) = post_at_ref(
            &owner,
            "/api/v1/reviews",
            json!({
                "title": action, "drafts": [{"draft_id": draft["draft"]["draft_id"],
                    "expected_draft_version": draft["draft"]["version"]}]
            }),
            commit,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{review}");
        assert_eq!(
            inbox(&reader)
                .await
                .items
                .first()
                .map(|item| item.version)
                .unwrap_or(0),
            index as i64,
            "Submitting a Draft does not publish its content to consuming projects."
        );
        let review_id = review["review"]["review_id"].as_str().unwrap();
        let (status, merged) = post_at_ref(
            &owner,
            &format!("/api/v1/reviews/{review_id}/merges"),
            json!({"expected_review_version": review["review"]["version"]}),
            commit,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{merged}");
        let received = inbox(&reader).await;
        assert_eq!(
            received.items.len(),
            1,
            "The unrelated project must receive no shared update."
        );
        assert_eq!(received.items[0].kind, "shared_update");
        assert_eq!(received.items[0].project_id, projects[0]);
        assert_eq!(received.items[0].version, index as i64 + 1);
        assert!(
            inbox(&owner).await.items.is_empty(),
            "The publisher does not receive their own notification."
        );
    }
    db.shutdown().await;
}
