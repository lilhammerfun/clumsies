//! Administrative HTTP contracts, filtering, membership policy, and audit persistence.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde::Serialize;
use server::app::audit_event::dto::AuditEventListResponse;
use server::app::auth::dto::OidcProviderStatus;
use server::app::memory::dto::MemoryExport;
use server::app::organization::dto::{
    AdminOrg, CreateMemberRequest, Member, MemberListResponse, MemberStatus, OrgRole,
    UpdateAdminOrgRequest, UpdateMemberRequest,
};
use server::app::project::dto::{
    AdminProject, AdminProjectListResponse, CreateProjectMemberRequest, CreateProjectRequest,
    ProjectMember, ProjectMemberListResponse, ProjectRole, UpdateProjectMemberRequest,
    UpdateProjectRequest,
};
use server::app::token::dto::{AccessTokenKind, AccessTokenListResponse};
use server::dto::DeleteResult;
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn owner_can_operate_the_complete_admin_contract() {
    let postgres = common::migrated_postgres().await;
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Acme Memory",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Default",
    )
    .await;
    let (app, _) = common::authenticated_router(postgres.pool.clone()).await;

    let org: AdminOrg = get_json(app.clone(), "/api/v1/admin/org").await;
    assert_eq!(org.org_id, bootstrap.org_id);
    let provider: OidcProviderStatus =
        get_json(app.clone(), "/api/v1/admin/identity-provider").await;
    assert!(provider.configured);

    let org: AdminOrg = patch_json(
        app.clone(),
        "/api/v1/admin/org",
        org.revision,
        &UpdateAdminOrgRequest {
            name: Some("Acme Knowledge".to_owned()),
            allowed_email_domains: Some(vec!["EXAMPLE.COM".to_owned()]),
        },
    )
    .await;
    assert_eq!(org.name, "Acme Knowledge");
    assert_eq!(org.allowed_email_domains, vec!["example.com"]);

    let project: AdminProject = post_json(
        app.clone(),
        "/api/v1/admin/projects",
        &CreateProjectRequest {
            name: "Research".to_owned(),
            description: Some("Shared research memory".to_owned()),
        },
    )
    .await;
    assert_eq!(project.name, "Research");
    assert_eq!(project.member_count, 1);
    let project: AdminProject = patch_json(
        app.clone(),
        &format!("/api/v1/admin/projects/{}", project.project_id),
        project.revision,
        &UpdateProjectRequest {
            name: Some("Research Lab".to_owned()),
            description: None,
        },
    )
    .await;
    assert_eq!(project.name, "Research Lab");

    let member: Member = post_json(
        app.clone(),
        "/api/v1/admin/members",
        &CreateMemberRequest {
            email: "member@example.com".to_owned(),
            role: OrgRole::Member,
        },
    )
    .await;
    assert_eq!(member.status, MemberStatus::Invited);

    let members: MemberListResponse = get_json(app.clone(), "/api/v1/admin/members").await;
    assert_eq!(members.items.len(), 2);
    assert!(
        members
            .items
            .iter()
            .find(|item| item.user_id == bootstrap.user_id)
            .unwrap()
            .external_identity_bound
    );
    assert!(
        !members
            .items
            .iter()
            .find(|item| item.user_id == member.user_id)
            .unwrap()
            .external_identity_bound
    );

    let member: Member = patch_json(
        app.clone(),
        &format!("/api/v1/admin/members/{}", member.user_id),
        member.revision,
        &UpdateMemberRequest {
            role: Some(OrgRole::Admin),
            status: Some(MemberStatus::Active),
        },
    )
    .await;
    assert_eq!(member.role, OrgRole::Admin);
    assert_eq!(member.status, MemberStatus::Active);

    let project_member: ProjectMember = post_json(
        app.clone(),
        &format!("/api/v1/admin/projects/{}/members", bootstrap.project_id),
        &CreateProjectMemberRequest {
            user_id: member.user_id.clone(),
            role: ProjectRole::Member,
        },
    )
    .await;
    assert_eq!(project_member.project_id, bootstrap.project_id);
    assert_eq!(project_member.user.user_id, member.user_id);
    assert_eq!(project_member.user.role, "admin");
    assert_eq!(project_member.role, ProjectRole::Member);

    let project_members: ProjectMemberListResponse = get_json(
        app.clone(),
        &format!(
            "/api/v1/admin/projects/{}/members?role=member",
            bootstrap.project_id
        ),
    )
    .await;
    assert_eq!(project_members.items, vec![project_member.clone()]);

    let project_member: ProjectMember = patch_without_revision(
        app.clone(),
        &format!(
            "/api/v1/admin/projects/{}/members/{}",
            bootstrap.project_id, member.user_id
        ),
        &UpdateProjectMemberRequest {
            role: ProjectRole::Admin,
        },
    )
    .await;
    assert_eq!(project_member.role, ProjectRole::Admin);

    let duplicate = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/admin/projects/{}/members",
                    bootstrap.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateProjectMemberRequest {
                        user_id: member.user_id.clone(),
                        role: ProjectRole::Member,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    let duplicate_body: serde_json::Value = decode_json(duplicate).await;
    assert_eq!(duplicate_body["error"]["code"], "already_exists");

    let projects: AdminProjectListResponse = get_json(app.clone(), "/api/v1/admin/projects").await;
    assert_eq!(projects.items.len(), 2);
    let default_project = projects
        .items
        .iter()
        .find(|item| item.project_id == bootstrap.project_id)
        .unwrap();
    assert_eq!(default_project.member_count, 2);

    let first_page: AdminProjectListResponse =
        get_json(app.clone(), "/api/v1/admin/projects?limit=1").await;
    assert_eq!(first_page.items.len(), 1);
    assert!(first_page.page_info.has_more);
    let cursor = first_page
        .page_info
        .next_cursor
        .expect("a partial page must include the next cursor");
    let second_page: AdminProjectListResponse = get_json(
        app.clone(),
        &format!("/api/v1/admin/projects?limit=1&cursor={cursor}"),
    )
    .await;
    assert_eq!(second_page.items.len(), 1);
    assert!(!second_page.page_info.has_more);
    assert_ne!(
        first_page.items[0].project_id,
        second_page.items[0].project_id
    );

    let tokens: AccessTokenListResponse = get_json(app.clone(), "/api/v1/admin/tokens").await;
    let refresh_token_id = tokens
        .items
        .iter()
        .find(|token| token.kind == AccessTokenKind::Refresh)
        .expect("OIDC login should issue a refresh token")
        .token_id
        .clone();
    let deleted_token: DeleteResult = delete_json(
        app.clone(),
        &format!("/api/v1/admin/tokens/{refresh_token_id}"),
        None,
    )
    .await;
    assert_eq!(deleted_token.id, refresh_token_id);

    let deleted_project_member: DeleteResult = delete_json(
        app.clone(),
        &format!(
            "/api/v1/admin/projects/{}/members/{}",
            bootstrap.project_id, member.user_id
        ),
        None,
    )
    .await;
    assert_eq!(deleted_project_member.id, member.user_id);

    let deleted_project: DeleteResult = delete_json(
        app.clone(),
        &format!("/api/v1/admin/projects/{}", project.project_id),
        Some(project.revision),
    )
    .await;
    assert_eq!(deleted_project.id, project.project_id);

    sqlx::query(
        "INSERT INTO audit_events (event_id, org_id, action, target_type)
         VALUES ('evt_system_identity_test', $1, 'system.test', 'test')",
    )
    .bind(&bootstrap.org_id)
    .execute(&postgres.pool)
    .await
    .unwrap();

    let audit_events: AuditEventListResponse =
        get_json(app.clone(), "/api/v1/admin/audit-events").await;
    let system_event = audit_events
        .items
        .iter()
        .find(|event| event.event_id == "evt_system_identity_test")
        .expect("audit events without an actor must remain visible");
    assert_eq!(system_event.actor_user_id, None);
    assert_eq!(system_event.actor_display_name, None);
    assert_eq!(system_event.actor_email, None);
    let owner_event = audit_events
        .items
        .iter()
        .find(|event| event.action == "admin.org_updated")
        .expect("organization update must record the owner identity");
    assert_eq!(
        owner_event.actor_user_id.as_deref(),
        Some(bootstrap.user_id.as_str())
    );
    assert_eq!(owner_event.actor_display_name.as_deref(), Some("Owner"));
    assert_eq!(
        owner_event.actor_email.as_deref(),
        Some("owner@example.com")
    );
    assert!(
        audit_events
            .items
            .iter()
            .any(|event| event.action == "admin.org_updated")
    );
    assert!(
        audit_events
            .items
            .iter()
            .any(|event| event.action == "admin.token_revoked")
    );
    assert!(
        audit_events
            .items
            .iter()
            .any(|event| event.action == "admin.project_member_created")
    );
    assert!(
        audit_events
            .items
            .iter()
            .any(|event| event.action == "admin.project_member_deleted")
    );
    assert!(
        audit_events
            .items
            .iter()
            .any(|event| event.action == "admin.project_created")
    );
    assert!(
        audit_events
            .items
            .iter()
            .any(|event| event.action == "admin.project_deleted")
    );

    let deleted_member: DeleteResult = delete_json(
        app,
        &format!("/api/v1/admin/members/{}", member.user_id),
        Some(member.revision),
    )
    .await;
    assert_eq!(deleted_member.id, member.user_id);
    postgres.shutdown().await;
}

#[tokio::test]
async fn admin_search_filters_before_paging_and_resolves_current_audit_targets() {
    let postgres = common::migrated_postgres().await;
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Search Organization",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Atlas Needle",
    )
    .await;
    let (app, _) = common::authenticated_router(postgres.pool.clone()).await;
    sqlx::query(
        "INSERT INTO users (user_id, email, display_name, role, status) VALUES
         ('usr_search_alpha', 'alpha@example.com', 'Search Person', 'member', 'invited'),
         ('usr_search_beta', 'search@example.com', 'Beta', 'member', 'invited'),
         ('usr_search_other', 'other@example.com', 'Other', 'member', 'invited')",
    )
    .execute(&postgres.pool)
    .await
    .unwrap();

    let first: MemberListResponse =
        get_json(app.clone(), "/api/v1/admin/members?limit=1&q=%20SeArCh%20").await;
    assert_eq!(first.items[0].user_id, "usr_search_alpha");
    let next = first.page_info.next_cursor.unwrap();
    let second: MemberListResponse = get_json(
        app.clone(),
        &format!("/api/v1/admin/members?limit=1&q=search&cursor={next}"),
    )
    .await;
    assert_eq!(second.items[0].user_id, "usr_search_beta");
    assert!(!second.page_info.has_more);
    let blank: MemberListResponse = get_json(app.clone(), "/api/v1/admin/members?q=%20%20").await;
    assert_eq!(blank.items.len(), 4);
    let literal: MemberListResponse = get_json(app.clone(), "/api/v1/admin/members?q=%25").await;
    assert!(
        literal.items.is_empty(),
        "search must not interpret SQL wildcards"
    );

    sqlx::query(
        "INSERT INTO audit_events
         (event_id, org_id, actor_user_id, action, target_type, target_id, created_at) VALUES
         ('evt_search_org', $1, $2, 'admin.org_updated', 'org', $1, '2030-01-01'),
         ('evt_search_project', $1, $2, 'admin.project_updated', 'project', $3, '2030-01-02'),
         ('evt_search_membership', $1, $2, 'admin.project_member_created', 'project_member',
            $3 || ':usr_search_alpha', '2030-01-03'),
         ('evt_search_user', $1, $2, 'admin.member_updated', 'user', 'usr_search_alpha', '2030-01-04'),
         ('evt_search_deleted', $1, $2, 'admin.project_deleted', 'project', 'prj_removed', '2030-01-05')",
    )
    .bind(&bootstrap.org_id)
    .bind(&bootstrap.user_id)
    .bind(&bootstrap.project_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    let first: AuditEventListResponse = get_json(
        app.clone(),
        "/api/v1/admin/audit-events?limit=1&q=%20ATLAS%20",
    )
    .await;
    assert_eq!(first.items[0].event_id, "evt_search_membership");
    assert_eq!(
        first.items[0].target_display_name.as_deref(),
        Some("Atlas Needle · Search Person")
    );
    let next = first.page_info.next_cursor.unwrap();
    let second: AuditEventListResponse = get_json(
        app.clone(),
        &format!("/api/v1/admin/audit-events?limit=1&q=atlas&cursor={next}"),
    )
    .await;
    assert_eq!(second.items[0].event_id, "evt_search_project");
    assert_eq!(
        second.items[0].target_display_name.as_deref(),
        Some("Atlas Needle")
    );
    assert!(!second.page_info.has_more);
    let all: AuditEventListResponse = get_json(
        app.clone(),
        "/api/v1/admin/audit-events?q=owner%40example.com",
    )
    .await;
    for (id, name) in [
        ("evt_search_org", Some("Search Organization")),
        ("evt_search_user", Some("Search Person")),
        ("evt_search_deleted", None),
    ] {
        let event = all.items.iter().find(|event| event.event_id == id).unwrap();
        assert_eq!(event.target_display_name.as_deref(), name);
    }
    let action: AuditEventListResponse = get_json(
        app.clone(),
        "/api/v1/admin/audit-events?q=admin.project_deleted",
    )
    .await;
    assert_eq!(action.items.len(), 1);
    assert_eq!(action.items[0].event_id, "evt_search_deleted");

    let (member_app, _) = common::authenticated_router_as(
        postgres.pool.clone(),
        "alpha@example.com",
        "subject-search-alpha",
        "Search Person",
    )
    .await;
    for path in [
        "/api/v1/admin/members?q=search",
        "/api/v1/admin/audit-events?q=atlas",
    ] {
        let response = member_app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    postgres.shutdown().await;
}

#[tokio::test]
async fn unknown_admin_project_is_not_disclosed() {
    let postgres = common::migrated_postgres().await;
    common::initialize_installation(
        postgres.pool.clone(),
        "Primary",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Primary",
    )
    .await;
    let (app, _) = common::authenticated_router(postgres.pool.clone()).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/admin/projects/prj_unknown/members")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    postgres.shutdown().await;
}

#[tokio::test]
async fn admin_updates_reject_stale_revisions_and_preserve_the_last_owner() {
    let postgres = common::migrated_postgres().await;
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Primary",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Primary",
    )
    .await;
    let (app, _) = common::authenticated_router(postgres.pool.clone()).await;

    let project: AdminProject = post_json(
        app.clone(),
        "/api/v1/admin/projects",
        &CreateProjectRequest {
            name: "Stable".to_owned(),
            description: None,
        },
    )
    .await;
    let stale_project = json_response(
        app.clone(),
        "PATCH",
        &format!("/api/v1/admin/projects/{}", project.project_id),
        Some(project.revision + 1),
        &UpdateProjectRequest {
            name: Some("Overwritten".to_owned()),
            description: None,
        },
    )
    .await;
    assert_eq!(stale_project.status(), StatusCode::CONFLICT);
    let stale_body: serde_json::Value = decode_json(stale_project).await;
    assert_eq!(stale_body["error"]["code"], "version_conflict");
    let unchanged: AdminProject = get_json(
        app.clone(),
        &format!("/api/v1/admin/projects/{}", project.project_id),
    )
    .await;
    assert_eq!(unchanged.name, "Stable");

    let members: MemberListResponse = get_json(app.clone(), "/api/v1/admin/members").await;
    let owner = members
        .items
        .into_iter()
        .find(|member| member.user_id == bootstrap.user_id)
        .expect("bootstrap owner must be listed");
    let demotion = json_response(
        app.clone(),
        "PATCH",
        &format!("/api/v1/admin/members/{}", owner.user_id),
        Some(owner.revision),
        &UpdateMemberRequest {
            role: Some(OrgRole::Member),
            status: None,
        },
    )
    .await;
    assert_eq!(demotion.status(), StatusCode::BAD_REQUEST);
    let demotion_body: serde_json::Value = decode_json(demotion).await;
    assert_eq!(demotion_body["error"]["code"], "invalid_request");

    let self_disable = json_response(
        app.clone(),
        "PATCH",
        &format!("/api/v1/admin/members/{}", owner.user_id),
        Some(owner.revision),
        &UpdateMemberRequest {
            role: None,
            status: Some(MemberStatus::Disabled),
        },
    )
    .await;
    assert_eq!(self_disable.status(), StatusCode::BAD_REQUEST);
    let self_disable_body: serde_json::Value = decode_json(self_disable).await;
    assert_eq!(self_disable_body["error"]["code"], "invalid_request");

    let self_uninvite = json_response(
        app,
        "PATCH",
        &format!("/api/v1/admin/members/{}", owner.user_id),
        Some(owner.revision),
        &UpdateMemberRequest {
            role: None,
            status: Some(MemberStatus::Invited),
        },
    )
    .await;
    assert_eq!(self_uninvite.status(), StatusCode::BAD_REQUEST);
    let self_uninvite_body: serde_json::Value = decode_json(self_uninvite).await;
    assert_eq!(self_uninvite_body["error"]["code"], "invalid_request");
    postgres.shutdown().await;
}

#[tokio::test]
async fn admin_lists_reject_invalid_pagination() {
    let postgres = common::migrated_postgres().await;
    common::initialize_installation(
        postgres.pool.clone(),
        "Primary",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Primary",
    )
    .await;
    let (app, _) = common::authenticated_router(postgres.pool.clone()).await;

    for uri in [
        "/api/v1/admin/projects?limit=0",
        "/api/v1/admin/projects?limit=201",
        "/api/v1/admin/projects?limit=not-a-number",
        "/api/v1/admin/projects?cursor=not-a-cursor",
        "/api/v1/admin/projects?cursor=-1",
        "/api/v1/admin/projects/prj_unknown/members?role=owner",
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        let body: serde_json::Value = decode_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_request", "{uri}");
    }
    postgres.shutdown().await;
}

async fn get_json<T>(app: Router, uri: &str) -> T
where
    T: serde::de::DeserializeOwned,
{
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(response.status().is_success());
    decode_json(response).await
}

async fn post_json<TRequest, TResponse>(app: Router, uri: &str, request: &TRequest) -> TResponse
where
    TRequest: Serialize,
    TResponse: serde::de::DeserializeOwned,
{
    let response = json_response(app, "POST", uri, None, request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    decode_json(response).await
}

async fn patch_json<TRequest, TResponse>(
    app: Router,
    uri: &str,
    revision: i64,
    request: &TRequest,
) -> TResponse
where
    TRequest: Serialize,
    TResponse: serde::de::DeserializeOwned,
{
    request_json(app, "PATCH", uri, Some(revision), request).await
}

async fn patch_without_revision<TRequest, TResponse>(
    app: Router,
    uri: &str,
    request: &TRequest,
) -> TResponse
where
    TRequest: Serialize,
    TResponse: serde::de::DeserializeOwned,
{
    request_json(app, "PATCH", uri, None, request).await
}

async fn request_json<TRequest, TResponse>(
    app: Router,
    method: &str,
    uri: &str,
    revision: Option<i64>,
    request: &TRequest,
) -> TResponse
where
    TRequest: Serialize,
    TResponse: serde::de::DeserializeOwned,
{
    let response = json_response(app, method, uri, revision, request).await;
    assert!(response.status().is_success());
    decode_json(response).await
}

async fn json_response<TRequest>(
    app: Router,
    method: &str,
    uri: &str,
    revision: Option<i64>,
    request: &TRequest,
) -> axum::response::Response
where
    TRequest: Serialize,
{
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(revision) = revision {
        builder = builder.header("if-match", revision.to_string());
    }
    app.oneshot(
        builder
            .body(Body::from(serde_json::to_vec(request).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn delete_json<T>(app: Router, uri: &str, revision: Option<i64>) -> T
where
    T: serde::de::DeserializeOwned,
{
    let mut builder = Request::builder().method("DELETE").uri(uri);
    if let Some(revision) = revision {
        builder = builder.header("if-match", revision.to_string());
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    decode_json(response).await
}

async fn decode_json<T>(response: axum::response::Response) -> T
where
    T: serde::de::DeserializeOwned,
{
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

struct SeedMemory<'a> {
    pool: &'a sqlx::PgPool,
    org_id: &'a str,
    memory_id: &'a str,
    project_id: Option<&'a str>,
    scope: &'a str,
    path: &'a str,
    name: &'a str,
    description: &'a str,
}

async fn seed_memory(seed: SeedMemory<'_>) {
    let SeedMemory {
        pool,
        org_id,
        memory_id,
        project_id,
        scope,
        path,
        name,
        description,
    } = seed;
    sqlx::query(
        "INSERT INTO resources (
            resource_id, org_id, project_id, scope, resource_kind, path, name,
            status, revision, content_hash, body, description
         )
         VALUES ($1, $2, $3, $4, 'memory', $5, $6, 'active', 1, $7, $8, $9)",
    )
    .bind(memory_id)
    .bind(org_id)
    .bind(project_id)
    .bind(scope)
    .bind(path)
    .bind(name)
    .bind("a".repeat(64))
    .bind(format!("# {name}\n\nBody of {path}."))
    .bind(description)
    .execute(pool)
    .await
    .unwrap();
}

/// ISSUE-012 migration tooling: the admin memory-export must capture every
/// identity family (legacy ctx_/rul_ ids and historical paths verbatim),
/// descriptions, active drafts with their operations, org selections and
/// personal bundles — the data the migration verification compares against.
#[tokio::test]
async fn memory_export_contains_verifiable_full_state() {
    let postgres = common::migrated_postgres().await;
    // This export fixture intentionally represents a pre-cutover database.
    // The production migration installs these guards as NOT VALID around
    // existing legacy rows, so remove them before seeding that old state.
    sqlx::query("ALTER TABLE resources DROP CONSTRAINT resources_no_active_project_authority")
        .execute(&postgres.pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE drafts DROP CONSTRAINT drafts_no_active_project_authority")
        .execute(&postgres.pool)
        .await
        .unwrap();
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Acme Memory",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Default",
    )
    .await;
    let (app, _) = common::authenticated_router(postgres.pool.clone()).await;

    let org_memory_id = "ctx_legacy_org_policy";
    let project_memory_id = "rul_legacy_project_rule";
    let issue_memory_id = "mem_native_issue";
    seed_memory(SeedMemory {
        pool: &postgres.pool,
        org_id: &bootstrap.org_id,
        memory_id: org_memory_id,
        project_id: None,
        scope: "org",
        path: "context/org-policy.md",
        name: "Org Policy",
        description: "Org-wide policy memory",
    })
    .await;
    seed_memory(SeedMemory {
        pool: &postgres.pool,
        org_id: &bootstrap.org_id,
        memory_id: project_memory_id,
        project_id: Some(&bootstrap.project_id),
        scope: "project",
        path: "rules/project-rule.md",
        name: "Project Rule",
        description: "Project rule memory",
    })
    .await;
    seed_memory(SeedMemory {
        pool: &postgres.pool,
        org_id: &bootstrap.org_id,
        memory_id: issue_memory_id,
        project_id: Some(&bootstrap.project_id),
        scope: "project",
        path: "issues/ISSUE-012.md",
        name: "Issue 12",
        description: "Issue 12 memory",
    })
    .await;

    sqlx::query(
        "INSERT INTO drafts (
            draft_id, project_id, author_user_id, title, description, resource_scope,
            resource_kind, status, version, daemon_installation_id
         )
         VALUES ($1, $2, $3, $4, $5, 'project', 'memory', 'open', 3, 'daemon-export-test')",
    )
    .bind("draft_export_1")
    .bind(&bootstrap.project_id)
    .bind(&bootstrap.user_id)
    .bind("Export draft")
    .bind("Draft description")
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO draft_operations (
            operation_id, draft_id, action, resource_scope, resource_kind, target_id,
            path, new_path, content, ordinal
         )
         VALUES
            ('op_export_z_create', 'draft_export_1', 'create', 'project', 'memory',
             NULL, 'rules/new.md', NULL,
             '{\"description\": \"New memory\", \"content\": \"# New\"}'::jsonb, 1),
            ('op_export_a_update', 'draft_export_1', 'update', 'project', 'memory',
             'rul_legacy_project_rule', 'rules/project-rule.md', NULL,
             '{\"content\": \"# Updated\"}'::jsonb, 2)",
    )
    .execute(&postgres.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO project_org_selection_states (project_id) VALUES ($1)
         ON CONFLICT (project_id) DO NOTHING",
    )
    .bind(&bootstrap.project_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO project_org_resource_selections (project_id, resource_id)
         VALUES ($1, $2)",
    )
    .bind(&bootstrap.project_id)
    .bind(org_memory_id)
    .execute(&postgres.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO personal_bundles (bundle_id, owner_user_id, name, description, revision)
         VALUES ('bundle_export_1', $1, 'Export bundle', 'Bundle description', 2)",
    )
    .bind(&bootstrap.user_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO personal_bundle_items (bundle_id, resource_id, position)
         VALUES ('bundle_export_1', $1, 0), ('bundle_export_1', $2, 1)",
    )
    .bind(org_memory_id)
    .bind(project_memory_id)
    .execute(&postgres.pool)
    .await
    .unwrap();

    let export: MemoryExport = get_json(app.clone(), "/api/v1/admin/memory-export").await;
    assert_eq!(export.org_id, bootstrap.org_id);

    assert_eq!(export.memories.len(), 3, "{:#?}", export.memories);
    let org_memory = export
        .memories
        .iter()
        .find(|item| item.memory_id == org_memory_id)
        .expect("legacy ctx_ id emitted verbatim");
    assert_eq!(org_memory.scope, "org");
    assert_eq!(org_memory.path, "context/org-policy.md");
    assert_eq!(org_memory.description, "Org-wide policy memory");
    let project_memory = export
        .memories
        .iter()
        .find(|item| item.memory_id == project_memory_id)
        .expect("legacy rul_ id emitted verbatim");
    assert_eq!(project_memory.scope, "project");
    assert_eq!(
        project_memory.project_id.as_deref(),
        Some(bootstrap.project_id.as_str())
    );
    assert_eq!(project_memory.description, "Project rule memory");
    let issue_memory = export
        .memories
        .iter()
        .find(|item| item.memory_id == issue_memory_id)
        .expect("historical memory path captured");
    assert_eq!(issue_memory.path, "issues/ISSUE-012.md");
    assert_eq!(issue_memory.status, "active");

    assert_eq!(export.drafts.len(), 1, "{:#?}", export.drafts);
    let draft = &export.drafts[0];
    assert_eq!(draft.draft_id, "draft_export_1");
    assert_eq!(draft.title, "Export draft");
    assert_eq!(draft.description, "Draft description");
    assert_eq!(draft.version, 3);
    assert_eq!(draft.operations.len(), 2);
    assert_eq!(draft.operations[0]["action"], "create");
    assert_eq!(draft.operations[0]["content"]["description"], "New memory");
    assert_eq!(draft.operations[1]["target_id"], "rul_legacy_project_rule");

    assert_eq!(export.selections.len(), 1);
    assert_eq!(export.selections[0].project_id, bootstrap.project_id);
    assert_eq!(
        export.selections[0].resource_ids,
        vec![org_memory_id.to_owned()]
    );

    assert_eq!(export.bundles.len(), 1);
    assert_eq!(export.bundles[0].name, "Export bundle");
    assert_eq!(export.bundles[0].owner_user_id, bootstrap.user_id);
    assert_eq!(
        export.bundles[0].resource_ids,
        vec![org_memory_id.to_owned(), project_memory_id.to_owned()]
    );
    postgres.shutdown().await;
}
