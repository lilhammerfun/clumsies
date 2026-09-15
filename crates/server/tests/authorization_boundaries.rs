mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use server::api::{
    CreateDraftRequest, CreateProjectRequest, DraftDetail, DraftOperationAction,
    DraftOperationBatchItem, DraftOperationBatchRequest, DraftOperationInput, DraftResourceContent,
    DraftResourceRef, ResourceScope,
};
use server::repository::ServerRepository;
use tower::ServiceExt;

#[tokio::test]
async fn bearer_identity_enforces_personal_and_project_boundaries() {
    let postgres = common::migrated_postgres().await;
    let repo = ServerRepository::new(postgres.pool.clone());
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Acme Memory",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Shared Project",
    )
    .await;
    let member_id = "usr_member".to_owned();
    sqlx::query(
        "INSERT INTO users (user_id, email, display_name, role, status)
         VALUES ($1, 'member@example.com', 'Member', 'member', 'active')",
    )
    .bind(&member_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(&bootstrap.project_id)
    .bind(&member_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    let private_project_id = repo
        .create_project(&bootstrap.org_id, "Owner Only", "")
        .await
        .unwrap();
    let selected_org_memory_id = repo
        .create_org_context(&bootstrap.org_id, "context/selected.md", "# Selected")
        .await
        .unwrap();
    let unselected_org_memory_id = repo
        .create_org_context(&bootstrap.org_id, "context/unselected.md", "# Unselected")
        .await
        .unwrap();
    repo.select_org_resource_for_project(&bootstrap.project_id, &selected_org_memory_id)
        .await
        .unwrap();

    let (owner_app, _) = common::authenticated_router(postgres.pool.clone()).await;
    let draft_response = owner_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/drafts")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateDraftRequest {
                        daemon_installation_id: "daemon_owner".to_owned(),
                        project_id: bootstrap.project_id.clone(),
                        base_commit_id: None,
                        title: "Owner draft".to_owned(),
                        description: None,
                        resource: DraftResourceRef {
                            scope: ResourceScope::Org,
                            id: None,
                            path: Some("context/private.md".to_owned()),
                        },
                        operations: Vec::new(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(draft_response.status(), StatusCode::OK);
    let owner_draft: DraftDetail = serde_json::from_slice(
        &to_bytes(draft_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let (member_app, _) = common::authenticated_router_as(
        postgres.pool.clone(),
        "member@example.com",
        "oidc-subject-member",
        "Member",
    )
    .await;

    let shared_project = member_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{}", bootstrap.project_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shared_project.status(), StatusCode::OK);

    let private_project = member_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{private_project_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(private_project.status(), StatusCode::NOT_FOUND);

    let private_draft = member_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/drafts/{}", owner_draft.draft.draft_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(private_draft.status(), StatusCode::NOT_FOUND);

    let operation_batch_body = |draft_id: &str| {
        serde_json::to_vec(&DraftOperationBatchRequest {
            daemon_installation_id: "daemon_member_batch".to_owned(),
            operations: vec![DraftOperationBatchItem {
                local_operation_id: format!("local-{draft_id}"),
                draft_id: draft_id.to_owned(),
                expected_draft_version: owner_draft.draft.version,
                operation: DraftOperationInput {
                    action: DraftOperationAction::Create,
                    resource: owner_draft.draft.resource.clone(),
                    content: Some(DraftResourceContent {
                        description: None,
                        content: "# Private".to_owned(),
                    }),
                    new_path: None,
                },
            }],
        })
        .unwrap()
    };
    let private_draft_batch = member_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/draft-operation-batches")
                .header("content-type", "application/json")
                .body(Body::from(operation_batch_body(
                    &owner_draft.draft.draft_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(private_draft_batch.status(), StatusCode::NOT_FOUND);

    let missing_draft_batch = member_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/draft-operation-batches")
                .header("content-type", "application/json")
                .body(Body::from(operation_batch_body("drf_missing")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_draft_batch.status(), StatusCode::NOT_FOUND);

    let org_draft = member_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/drafts")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateDraftRequest {
                        daemon_installation_id: "daemon_member".to_owned(),
                        project_id: bootstrap.project_id.clone(),
                        base_commit_id: None,
                        title: "New organization memory proposal".to_owned(),
                        description: None,
                        resource: DraftResourceRef {
                            scope: ResourceScope::Org,
                            id: None,
                            path: Some("context/org.md".to_owned()),
                        },
                        operations: Vec::new(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(org_draft.status(), StatusCode::OK);

    let selected_org_draft = member_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/drafts")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateDraftRequest {
                        daemon_installation_id: "daemon_member".to_owned(),
                        project_id: bootstrap.project_id.clone(),
                        base_commit_id: None,
                        title: "Selected organization memory proposal".to_owned(),
                        description: None,
                        resource: DraftResourceRef {
                            scope: ResourceScope::Org,
                            id: Some(selected_org_memory_id),
                            path: None,
                        },
                        operations: Vec::new(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_org_draft.status(), StatusCode::OK);

    let unselected_org_draft = member_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/drafts")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateDraftRequest {
                        daemon_installation_id: "daemon_member".to_owned(),
                        project_id: bootstrap.project_id.clone(),
                        base_commit_id: None,
                        title: "Unselected organization memory proposal".to_owned(),
                        description: None,
                        resource: DraftResourceRef {
                            scope: ResourceScope::Org,
                            id: Some(unselected_org_memory_id),
                            path: None,
                        },
                        operations: Vec::new(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unselected_org_draft.status(), StatusCode::BAD_REQUEST);

    let create_project = member_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects")
                .header("content-type", "application/json")
                .header("idempotency-key", "member-create-project")
                .body(Body::from(
                    serde_json::to_vec(&CreateProjectRequest {
                        name: "Member Project".to_owned(),
                        description: None,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_project.status(), StatusCode::CREATED);
    let created: server::api::Project = serde_json::from_slice(
        &to_bytes(create_project.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let project_id = &created.project_id;
    let (_, me) = project_request(&member_app, "GET", "/api/v1/me", None).await;
    assert!(
        me["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "project:create")
    );
    assert!(
        !me["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "admin:write")
    );
    assert!(
        me["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["project_id"] == *project_id && p["role"] == "admin")
    );

    // A project creator can use both project editing routes, but gains no authority over other projects.
    for prefix in ["/api/v1/projects", "/api/v1/admin/projects"] {
        let (status, _) = project_request(
            &member_app,
            "PATCH",
            &format!("{prefix}/{project_id}"),
            Some(serde_json::json!({"description": "Owned by a member"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    for (method, path, body) in [
        (
            "PATCH",
            format!("/api/v1/projects/{}", bootstrap.project_id),
            serde_json::json!({"name":"Not allowed"}),
        ),
        (
            "DELETE",
            format!("/api/v1/admin/projects/{}", bootstrap.project_id),
            serde_json::json!({}),
        ),
        (
            "POST",
            format!("/api/v1/admin/projects/{}/members", bootstrap.project_id),
            serde_json::json!({"user_id":bootstrap.user_id,"role":"admin"}),
        ),
        (
            "PATCH",
            format!(
                "/api/v1/admin/projects/{}/members/{member_id}",
                bootstrap.project_id
            ),
            serde_json::json!({"role":"admin"}),
        ),
        (
            "PUT",
            format!("/api/v1/projects/{}/org-selections", bootstrap.project_id),
            serde_json::json!({"resource_ids":[]}),
        ),
        (
            "GET",
            "/api/v1/admin/projects".to_owned(),
            serde_json::json!({}),
        ),
        (
            "GET",
            "/api/v1/admin/members".to_owned(),
            serde_json::json!({}),
        ),
        (
            "PATCH",
            "/api/v1/admin/org".to_owned(),
            serde_json::json!({"name":"Not allowed"}),
        ),
        (
            "GET",
            format!(
                "/api/v1/admin/projects/{}/member-candidates",
                bootstrap.project_id
            ),
            serde_json::json!({}),
        ),
    ] {
        let (status, _) = project_request(&member_app, method, &path, Some(body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    let (status, _) = project_request(
        &member_app,
        "GET",
        &format!("/api/v1/admin/projects/{private_project_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let memory_id = repo
        .create_org_context(
            &bootstrap.org_id,
            "context/member-project.md",
            "# Initial memory",
        )
        .await
        .unwrap();
    let (status, selection) = project_request(
        &member_app,
        "PUT",
        &format!("/api/v1/projects/{project_id}/org-selections"),
        Some(serde_json::json!({"resource_ids":[memory_id]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(selection["memories"].as_array().unwrap().len(), 1);

    let (status, candidates) = project_request(
        &member_app,
        "GET",
        &format!("/api/v1/admin/projects/{project_id}/member-candidates?q=owner"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(candidates["items"][0]["user_id"], bootstrap.user_id);
    assert!(
        candidates["items"][0]
            .get("external_identity_bound")
            .is_none()
    );
    let (status, _) = project_request(
        &member_app,
        "POST",
        &format!("/api/v1/admin/projects/{project_id}/members"),
        Some(serde_json::json!({"user_id":bootstrap.user_id,"role":"member"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = project_request(
        &member_app,
        "DELETE",
        &format!(
            "/api/v1/admin/projects/{project_id}/members/{}",
            bootstrap.user_id
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = project_request(
        &member_app,
        "DELETE",
        &format!("/api/v1/admin/projects/{project_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let administer_project_members = member_app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/admin/projects/{}/members",
                    bootstrap.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(administer_project_members.status(), StatusCode::OK);
}

async fn project_request(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if matches!(method, "PATCH" | "PUT" | "DELETE") && !path.contains("/members/") {
        let current = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        if current.status() == StatusCode::OK {
            let value: serde_json::Value =
                serde_json::from_slice(&to_bytes(current.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            request = request.header("if-match", value["revision"].as_i64().unwrap().to_string());
        }
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(
                    body.unwrap_or(serde_json::json!({})).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    (status, body)
}
