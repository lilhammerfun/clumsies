mod common;

use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use server::api::{
    EpisodeEvidencePage, EpisodeEvidenceRecord, FinalizeProjectEpisodeRequest, ProjectEpisode,
    ProjectEpisodeListResponse, ProjectEpisodeStatus, ProjectEpisodeSummaryPolicy,
    UpdateProjectEpisodeSummaryPolicyRequest, project_episode_evidence_hash,
};
use server::auth::{AuthPrincipal, CredentialKind};
use server::repository::ServerRepository;
use time::macros::datetime;
use tower::ServiceExt;

fn finalize_request(content: &str) -> FinalizeProjectEpisodeRequest {
    let evidence = vec![EpisodeEvidenceRecord {
        sequence: 1,
        occurred_at: datetime!(2026-08-30 10:00 UTC),
        kind: "provider_record".to_owned(),
        content: content.to_owned(),
    }];
    FinalizeProjectEpisodeRequest {
        run_id: "arun_episode_test".to_owned(),
        host_session_id: Some("session_test".to_owned()),
        host: "codex".to_owned(),
        evidence_format: "codex_rollout_jsonl".to_owned(),
        evidence_format_revision: 1,
        activity_at: evidence[0].occurred_at,
        evidence_hash: project_episode_evidence_hash(&evidence).unwrap(),
        evidence,
    }
}

async fn finalize(
    app: Router,
    project_id: &str,
    idempotency_key: &str,
    request: &FinalizeProjectEpisodeRequest,
) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/projects/{project_id}/episodes/finalize"))
            .header("content-type", "application/json")
            .header("idempotency-key", idempotency_key)
            .body(Body::from(serde_json::to_vec(request).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn decode<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn project_episode_finalize_is_idempotent_and_project_isolated() {
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
    let (owner_app, _) = common::authenticated_router(postgres.pool.clone()).await;
    let request = finalize_request("a durable decision");
    let first = finalize(
        owner_app.clone(),
        &installation.project_id,
        "episode-finalize-1",
        &request,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first: ProjectEpisode = decode(first).await;
    assert!(
        first.episode_id.starts_with("episode_") && first.episode_id.len() == "episode_".len() + 32
    );
    assert_eq!(first.run_id, request.run_id);
    assert_eq!(first.evidence_hash, request.evidence_hash);
    assert!(matches!(
        first.status,
        ProjectEpisodeStatus::PendingSummary
            | ProjectEpisodeStatus::Active
            | ProjectEpisodeStatus::NoMemory
    ));

    let repeated = finalize(
        owner_app.clone(),
        &installation.project_id,
        "episode-finalize-1",
        &request,
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::OK);
    let repeated: ProjectEpisode = decode(repeated).await;
    assert_eq!(repeated.episode_id, first.episode_id);

    let same_run_new_key = finalize(
        owner_app.clone(),
        &installation.project_id,
        "episode-finalize-2",
        &request,
    )
    .await;
    assert_eq!(same_run_new_key.status(), StatusCode::OK);
    let same_run_new_key: ProjectEpisode = decode(same_run_new_key).await;
    assert_eq!(same_run_new_key.episode_id, first.episode_id);

    let conflicting = finalize_request("different evidence");
    let conflict = finalize(
        owner_app.clone(),
        &installation.project_id,
        "episode-finalize-3",
        &conflicting,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let list = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes?after_revision=0&limit=50",
                    installation.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list: ProjectEpisodeListResponse = decode(list).await;
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.next_cursor, Some(list.items[0].corpus_revision));
    assert!(list.corpus_revision >= list.items[0].corpus_revision);

    let mut newer_request = finalize_request("a newer durable decision");
    newer_request.run_id = "arun_episode_newer".to_owned();
    newer_request.evidence[0].occurred_at = datetime!(2026-08-30 11:00 UTC);
    newer_request.activity_at = newer_request.evidence[0].occurred_at;
    newer_request.evidence_hash = project_episode_evidence_hash(&newer_request.evidence).unwrap();
    let newer = finalize(
        owner_app.clone(),
        &installation.project_id,
        "episode-finalize-newer",
        &newer_request,
    )
    .await;
    assert_eq!(newer.status(), StatusCode::OK);
    let newer: ProjectEpisode = decode(newer).await;

    let oldest_change = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes?after_revision=0&limit=1",
                    installation.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let oldest_change: ProjectEpisodeListResponse = decode(oldest_change).await;
    assert_eq!(oldest_change.items[0].episode_id, first.episode_id);
    assert!(oldest_change.has_more);
    assert!(oldest_change.next_cursor.is_some());

    let most_recent = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes?recent=true&limit=1",
                    installation.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(most_recent.status(), StatusCode::OK);
    let most_recent: ProjectEpisodeListResponse = decode(most_recent).await;
    assert_eq!(most_recent.items[0].episode_id, newer.episode_id);
    assert!(most_recent.has_more);
    assert_eq!(most_recent.next_cursor, None);
    let before_delete_revision = most_recent.corpus_revision;

    let deleted_newer = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/projects/{}/episodes/{}",
                    installation.project_id, newer.episode_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_newer.status(), StatusCode::OK);
    let recent_after_delete = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes?recent=true&limit=1",
                    installation.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let recent_after_delete: ProjectEpisodeListResponse = decode(recent_after_delete).await;
    assert_eq!(recent_after_delete.items[0].episode_id, first.episode_id);
    let deletion_change = owner_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes?after_revision={before_delete_revision}&limit=1",
                    installation.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let deletion_change: ProjectEpisodeListResponse = decode(deletion_change).await;
    assert_eq!(deletion_change.items[0].episode_id, newer.episode_id);
    assert_eq!(
        deletion_change.items[0].status,
        ProjectEpisodeStatus::Deleted
    );

    sqlx::query(
        "INSERT INTO users (user_id, email, display_name, role, status)
         VALUES ('usr_outsider', 'outsider@example.com', 'Outsider', 'member', 'active')",
    )
    .execute(&postgres.pool)
    .await
    .unwrap();
    let (outsider_app, _) = common::authenticated_router_as(
        postgres.pool.clone(),
        "outsider@example.com",
        "outsider-subject",
        "Outsider",
    )
    .await;
    let hidden = outsider_app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes",
                    installation.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn evidence_is_bounded_audited_and_deleted_with_its_project_episode() {
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
    let (app, token) = common::authenticated_router(postgres.pool.clone()).await;
    let request = finalize_request(&"\u{0001}\n\"\\😀".repeat(700));
    let response = finalize(
        app.clone(),
        &installation.project_id,
        "episode-evidence-page",
        &request,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let episode: ProjectEpisode = decode(response).await;

    let first_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes/{}/evidence?limit=1&max_bytes=1024",
                    installation.project_id, episode.episode_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_page.status(), StatusCode::OK);
    let first_page_body = to_bytes(first_page.into_body(), usize::MAX).await.unwrap();
    assert!(first_page_body.len() <= 1024);
    let first_page: EpisodeEvidencePage = serde_json::from_slice(&first_page_body).unwrap();
    assert!(first_page.untrusted);
    assert!(!first_page.items[0].content.is_empty());
    assert!(first_page.items[0].content.len() < request.evidence[0].content.len());
    assert!(!first_page.items[0].complete);
    let first_page_bytes = first_page.items[0].content.len();
    let cursor = first_page.next_cursor.unwrap();

    let second_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes/{}/evidence?limit=1&max_bytes=262144&cursor={cursor}",
                    installation.project_id, episode.episode_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_page.status(), StatusCode::OK);
    let second_page: EpisodeEvidencePage = decode(second_page).await;
    assert_eq!(second_page.items[0].byte_offset, first_page_bytes as i64);
    assert!(second_page.items[0].complete);
    assert!(!second_page.has_more);

    sqlx::query(
        "INSERT INTO users (user_id, email, display_name, role, status)
         VALUES ('usr_episode_member', 'episode-member@example.com',
                 'Episode Member', 'member', 'active')",
    )
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO project_members (project_id, user_id, role)
         VALUES ($1, 'usr_episode_member', 'member')",
    )
    .bind(&installation.project_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    let (member_app, _) = common::authenticated_router_as(
        postgres.pool.clone(),
        "episode-member@example.com",
        "oidc-subject-episode-member",
        "Episode Member",
    )
    .await;

    let member_list = member_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes?recent=true",
                    installation.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(member_list.status(), StatusCode::OK);
    let member_evidence = member_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes/{}/evidence",
                    installation.project_id, episode.episode_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(member_evidence.status(), StatusCode::OK);

    let policy_update = member_app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/projects/{}/episode-summary-policy",
                    installation.project_id
                ))
                .header("content-type", "application/json")
                .header("if-match", "1")
                .body(Body::from(r#"{"instructions":"member policy"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(policy_update.status(), StatusCode::FORBIDDEN);
    let preview = member_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/projects/{}/episodes/{}/summary-preview",
                    installation.project_id, episode.episode_id
                ))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::FORBIDDEN);
    let rebuild = member_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/projects/{}/episodes/{}/summary-rebuild",
                    installation.project_id, episode.episode_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rebuild.status(), StatusCode::FORBIDDEN);
    let member_delete = member_app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/projects/{}/episodes/{}",
                    installation.project_id, episode.episode_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(member_delete.status(), StatusCode::FORBIDDEN);

    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events
         WHERE org_id = $1 AND actor_user_id = $2
           AND action = 'project_episode.evidence_read'
           AND target_id = $3",
    )
    .bind(&installation.org_id)
    .bind(&token.user.user_id)
    .bind(&episode.episode_id)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 2);

    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/projects/{}/episodes/{}",
                    installation.project_id, episode.episode_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let evidence_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM project_episode_evidence WHERE episode_id = $1")
            .bind(&episode.episode_id)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(evidence_count, 0);

    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/episodes/{}/evidence",
                    installation.project_id, episode.episode_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::NOT_FOUND);

    sqlx::query("DELETE FROM projects WHERE project_id = $1")
        .bind(&installation.project_id)
        .execute(&postgres.pool)
        .await
        .unwrap();
    let episode_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM project_episodes WHERE episode_id = $1")
            .bind(&episode.episode_id)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(episode_count, 0);
}

#[tokio::test]
async fn episode_list_waits_for_a_coherent_corpus_snapshot() {
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
    let (app, token) = common::authenticated_router(postgres.pool.clone()).await;
    let episode = finalize(
        app,
        &installation.project_id,
        "episode-snapshot",
        &finalize_request("snapshot evidence"),
    )
    .await;
    assert_eq!(episode.status(), StatusCode::OK);
    let episode: ProjectEpisode = decode(episode).await;

    let mut writer = postgres.pool.begin().await.unwrap();
    let next_revision: i64 = sqlx::query_scalar(
        "UPDATE project_episode_states
         SET corpus_revision = corpus_revision + 1, updated_at = now()
         WHERE project_id = $1
         RETURNING corpus_revision",
    )
    .bind(&installation.project_id)
    .fetch_one(&mut *writer)
    .await
    .unwrap();

    let repository = ServerRepository::new(postgres.pool.clone());
    let principal = AuthPrincipal {
        user_id: token.user.user_id,
        org_id: token.org.org_id,
        session_id: "test-session".to_owned(),
        token_id: "test-token".to_owned(),
        role: token.user.role,
        credential_kind: CredentialKind::Bearer,
        csrf_token: None,
    };
    let project_id = installation.project_id.clone();
    let mut reader = tokio::spawn(async move {
        repository
            .list_project_episodes(&principal, &project_id, 0, 50, false)
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut reader)
            .await
            .is_err(),
        "the reader must wait for the in-flight corpus writer"
    );

    sqlx::query(
        "UPDATE project_episodes
         SET corpus_revision = $2, revision = revision + 1, updated_at = now()
         WHERE episode_id = $1",
    )
    .bind(&episode.episode_id)
    .bind(next_revision)
    .execute(&mut *writer)
    .await
    .unwrap();
    writer.commit().await.unwrap();

    let page = reader.await.unwrap().unwrap();
    assert_eq!(page.corpus_revision, next_revision);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].corpus_revision, next_revision);
}

#[tokio::test]
async fn summary_policy_is_versioned_without_changing_the_corpus() {
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
    let path = format!(
        "/api/v1/projects/{}/episode-summary-policy",
        installation.project_id
    );
    let initial = app
        .clone()
        .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    let initial: ProjectEpisodeSummaryPolicy = decode(initial).await;
    assert_eq!(initial.instructions, "");
    assert_eq!(initial.revision, 1);

    let updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&path)
                .header("content-type", "application/json")
                .header("if-match", "1")
                .body(Body::from(
                    serde_json::to_vec(&UpdateProjectEpisodeSummaryPolicyRequest {
                        instructions: "Emphasize unresolved production risks.".to_owned(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated: ProjectEpisodeSummaryPolicy = decode(updated).await;
    assert_eq!(updated.revision, 2);
    assert_eq!(
        updated.instructions,
        "Emphasize unresolved production risks."
    );
    let corpus_revision: i64 = sqlx::query_scalar(
        "SELECT corpus_revision FROM project_episode_states WHERE project_id = $1",
    )
    .bind(&installation.project_id)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(corpus_revision, 0);
}
