//! Bounded database query work for proposal and review read operations.

use server::app::draft::dto::{
    CreateDraftRequest, DraftOperationAction, DraftOperationInput, DraftResourceContent,
    DraftResourceRef,
};
use server::app::memory::dto::ResourceScope;
use server::error::ServerError;
use std::time::Duration;

mod common;

#[tokio::test]
async fn metadata_and_draft_reads_skip_payloads_and_ref_locks() {
    let postgres = common::migrated_postgres().await;
    let pool = postgres.pool.clone();
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Read Paths",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Read Paths",
    )
    .await;

    let org_memory = server::app::memory::create_org_context(
        &pool,
        &common::owner_principal(&pool).await,
        "context/selected.md",
        "# Selected",
    )
    .await
    .unwrap();
    server::app::memory::select_org_resource_for_project(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        &org_memory,
    )
    .await
    .unwrap();
    let project_head = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .expect("project selection should create a project commit");
    server::app::commit::get_commit_payload(
        &pool,
        &common::owner_principal(&pool).await,
        &project_head,
    )
    .await
    .unwrap();
    let org_head = server::app::commit::get_org_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id;

    let draft = server::app::draft::create_draft(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        CreateDraftRequest {
            daemon_installation_id: "daemon_read_paths".to_owned(),
            project_id: bootstrap.project_id.clone(),
            base_commit_id: org_head,
            title: "Create Organization memory".to_owned(),
            description: None,
            resource: DraftResourceRef {
                scope: ResourceScope::Org,
                id: None,
                path: Some("context/read-paths.md".to_owned()),
            },
            operations: vec![DraftOperationInput {
                action: DraftOperationAction::Create,
                resource: DraftResourceRef {
                    scope: ResourceScope::Org,
                    id: None,
                    path: Some("context/read-paths.md".to_owned()),
                },
                content: Some(DraftResourceContent {
                    description: None,
                    content: "# Read paths".to_owned(),
                }),
                new_path: None,
            }],
        },
    )
    .await
    .unwrap();

    let mut ref_lock = postgres.pool.begin().await.unwrap();
    sqlx::query_scalar::<_, String>(
        "SELECT ref_id
         FROM refs
         WHERE scope = 'org' AND org_id = $1 AND ref_name = 'refs/heads/main'
         FOR UPDATE",
    )
    .bind(&bootstrap.org_id)
    .fetch_one(&mut *ref_lock)
    .await
    .unwrap();

    let read = tokio::time::timeout(
        Duration::from_secs(3),
        server::app::draft::get_draft(
            &pool,
            &common::owner_principal(&pool).await,
            &draft.draft.draft_id,
        ),
    )
    .await;
    ref_lock.rollback().await.unwrap();
    assert!(
        read.is_ok(),
        "a draft read must not wait for a ref mutation lock"
    );
    read.unwrap().unwrap();

    sqlx::query(
        "DELETE FROM tree_entries
         WHERE tree_id = (SELECT tree_id FROM commits WHERE commit_id = $1)",
    )
    .bind(&project_head)
    .execute(&postgres.pool)
    .await
    .unwrap();

    let state = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap();
    assert_eq!(state.latest.unwrap().commit_id, project_head);

    let commits = server::app::commit::list_project_commits(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
    )
    .await
    .unwrap();
    assert!(
        commits
            .items
            .iter()
            .any(|commit| commit.commit_id == project_head)
    );
    assert!(matches!(
        server::app::commit::get_commit_payload(
            &pool,
            &common::owner_principal(&pool).await,
            &project_head
        )
        .await,
        Err(ServerError::InvalidRequest(_))
    ));
    postgres.shutdown().await;
}
