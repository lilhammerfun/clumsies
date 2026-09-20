//! Stable proposal operation ordering and atomic multi-proposal publication.

use server::app::auth::AuthPrincipal;
use server::app::draft::dto::{
    CreateDraftReconciliationCandidateRequest, CreateDraftRequest, DraftOperationAction,
    DraftOperationBatchItem, DraftOperationBatchRequest, DraftOperationInput, DraftResourceContent,
    DraftResourceRef,
};
use server::app::memory::dto::ResourceScope;
use server::app::review::dto::{
    CreateReviewDecisionRequest, CreateReviewMergeRequest, CreateReviewRequest, ReviewDecision,
    ReviewDraftRequest,
};
use server::error::ServerError;
use std::time::Duration;

mod common;

fn memory_content(content: &str) -> Option<DraftResourceContent> {
    Some(DraftResourceContent {
        description: None,
        content: content.to_owned(),
    })
}

#[tokio::test]
async fn multi_draft_review_merges_every_file_in_one_commit() {
    let postgres = common::migrated_postgres().await;
    let pool = postgres.pool.clone();
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Directory Review",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Directory Review",
    )
    .await;
    let head = server::app::commit::get_org_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id;

    let mut drafts = Vec::new();
    for (index, path) in [
        "skills/coding/SKILL.md",
        "skills/coding/references/workflow.md",
    ]
    .into_iter()
    .enumerate()
    {
        drafts.push(
            server::app::draft::create_draft(
                &pool,
                &common::principal(&pool, &bootstrap.user_id).await,
                CreateDraftRequest {
                    daemon_installation_id: format!("daemon_directory_{index}"),
                    project_id: bootstrap.project_id.clone(),
                    base_commit_id: head.clone(),
                    title: format!("Create {path}"),
                    description: None,
                    resource: DraftResourceRef {
                        scope: ResourceScope::Org,
                        id: None,
                        path: Some(path.to_owned()),
                    },
                    operations: vec![DraftOperationInput {
                        action: DraftOperationAction::Create,
                        resource: DraftResourceRef {
                            scope: ResourceScope::Org,
                            id: None,
                            path: Some(path.to_owned()),
                        },
                        content: memory_content(&format!("# File {index}")),
                        new_path: None,
                    }],
                },
            )
            .await
            .unwrap(),
        );
    }

    let first_submission = server::app::review::create_review(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        head.as_deref(),
        CreateReviewRequest {
            drafts: vec![ReviewDraftRequest {
                draft_id: drafts[0].draft.draft_id.clone(),
                expected_draft_version: drafts[0].draft.version,
                candidate_id: None,
                resolved_state: None,
            }],
            title: Some("Create coding skill".to_owned()),
            description: None,
        },
    )
    .await
    .unwrap();
    let rejected = server::app::review::create_review_decision(
        &pool,
        &first_submission.review.review_id,
        &common::principal(&pool, &bootstrap.user_id).await,
        CreateReviewDecisionRequest {
            decision: ReviewDecision::Rejected,
            expected_review_version: first_submission.review.version,
            body: Some("Include every file in the directory.".to_owned()),
        },
    )
    .await
    .unwrap();
    let detail = server::app::review::create_review(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        head.as_deref(),
        CreateReviewRequest {
            drafts: vec![
                ReviewDraftRequest {
                    draft_id: rejected.drafts[0].draft.draft_id.clone(),
                    expected_draft_version: rejected.drafts[0].draft.version,
                    candidate_id: None,
                    resolved_state: None,
                },
                ReviewDraftRequest {
                    draft_id: drafts[1].draft.draft_id.clone(),
                    expected_draft_version: drafts[1].draft.version,
                    candidate_id: None,
                    resolved_state: None,
                },
            ],
            title: Some("Create coding skill".to_owned()),
            description: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(detail.review.review_id, first_submission.review.review_id);
    assert_eq!(detail.review.draft_ids.len(), 2);
    assert_eq!(detail.drafts.len(), 2);
    assert!(
        detail
            .drafts
            .iter()
            .all(|item| item.draft.status == server::app::draft::dto::DraftStatus::Submitted)
    );

    let principal = AuthPrincipal {
        user_id: bootstrap.user_id.clone(),
        org_id: bootstrap.org_id.clone(),
        session_id: "session_review_list".to_owned(),
        token_id: "token_review_list".to_owned(),
        role: "owner".to_owned(),
    };
    let mut blob_lock = postgres.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE blobs IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blob_lock)
        .await
        .unwrap();
    let draft_list = tokio::time::timeout(
        Duration::from_secs(3),
        server::app::draft::list_drafts(
            &pool,
            &common::principal(&pool, &bootstrap.user_id).await,
            Some(&bootstrap.project_id),
        ),
    )
    .await;
    let reviews = tokio::time::timeout(
        Duration::from_secs(3),
        server::app::review::list_reviews(&pool, &principal, Some(&bootstrap.project_id)),
    )
    .await;
    blob_lock.rollback().await.unwrap();
    let draft_list = draft_list
        .expect("a draft list must not wait for a blob payload table lock")
        .unwrap();
    assert_eq!(draft_list.items.len(), 2);
    let reviews = reviews
        .expect("a review list must not wait for a blob payload table lock")
        .unwrap();
    assert_eq!(reviews.items, vec![detail.review.clone()]);

    let approved = server::app::review::create_review_decision(
        &pool,
        &detail.review.review_id,
        &common::principal(&pool, &bootstrap.user_id).await,
        CreateReviewDecisionRequest {
            decision: ReviewDecision::Approved,
            expected_review_version: detail.review.version,
            body: None,
        },
    )
    .await
    .unwrap();
    let merged = server::app::review::create_review_merge(
        &pool,
        &detail.review.review_id,
        &common::principal(&pool, &bootstrap.user_id).await,
        head.as_deref(),
        CreateReviewMergeRequest {
            expected_review_version: approved.review.version,
        },
    )
    .await
    .unwrap();
    assert_eq!(merged.applied_operation_count, 2);

    let commit = server::app::commit::get_commit_payload(
        &pool,
        &common::owner_principal(&pool).await,
        merged.commit_id.as_deref().unwrap(),
    )
    .await
    .unwrap();
    let paths = commit
        .tree
        .entries
        .iter()
        .filter_map(|entry| entry.path.as_deref())
        .collect::<Vec<_>>();
    assert!(paths.contains(&"skills/coding/SKILL.md"));
    assert!(paths.contains(&"skills/coding/references/workflow.md"));
    postgres.shutdown().await;
}

#[tokio::test]
async fn multi_draft_review_reconciles_atomically() {
    let postgres = common::migrated_postgres().await;
    let pool = postgres.pool.clone();
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Atomic Directory Review",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Atomic Directory Review",
    )
    .await;
    let initial_head = server::app::commit::get_org_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id;

    let mut drafts = Vec::new();
    for (index, path) in [
        "context/first.md",
        "context/second.md",
        "context/advance-head.md",
    ]
    .into_iter()
    .enumerate()
    {
        drafts.push(
            server::app::draft::create_draft(
                &pool,
                &common::principal(&pool, &bootstrap.user_id).await,
                CreateDraftRequest {
                    daemon_installation_id: format!("daemon_atomic_review_{index}"),
                    project_id: bootstrap.project_id.clone(),
                    base_commit_id: initial_head.clone(),
                    title: format!("Create {path}"),
                    description: None,
                    resource: DraftResourceRef {
                        scope: ResourceScope::Org,
                        id: None,
                        path: Some(path.to_owned()),
                    },
                    operations: vec![DraftOperationInput {
                        action: DraftOperationAction::Create,
                        resource: DraftResourceRef {
                            scope: ResourceScope::Org,
                            id: None,
                            path: Some(path.to_owned()),
                        },
                        content: memory_content(&format!("# File {index}")),
                        new_path: None,
                    }],
                },
            )
            .await
            .unwrap(),
        );
    }

    approve_and_merge(
        &pool,
        &bootstrap.user_id,
        initial_head.as_deref(),
        &drafts[2].draft.draft_id,
        drafts[2].draft.version,
    )
    .await;
    let current_head = server::app::commit::get_org_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id;
    let first_candidate = server::app::draft::create_draft_reconciliation_candidate(
        &pool,
        &common::owner_principal(&pool).await,
        &drafts[0].draft.draft_id,
        CreateDraftReconciliationCandidateRequest {
            expected_draft_version: drafts[0].draft.version,
        },
    )
    .await
    .unwrap();
    let second_candidate = server::app::draft::create_draft_reconciliation_candidate(
        &pool,
        &common::owner_principal(&pool).await,
        &drafts[1].draft.draft_id,
        CreateDraftReconciliationCandidateRequest {
            expected_draft_version: drafts[1].draft.version,
        },
    )
    .await
    .unwrap();

    let before = server::app::draft::get_draft(
        &pool,
        &common::owner_principal(&pool).await,
        &drafts[0].draft.draft_id,
    )
    .await
    .unwrap();
    let error = server::app::review::create_review(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        current_head.as_deref(),
        CreateReviewRequest {
            drafts: vec![
                ReviewDraftRequest {
                    draft_id: drafts[0].draft.draft_id.clone(),
                    expected_draft_version: drafts[0].draft.version,
                    candidate_id: Some(first_candidate.candidate_id.clone()),
                    resolved_state: None,
                },
                ReviewDraftRequest {
                    draft_id: drafts[1].draft.draft_id.clone(),
                    expected_draft_version: drafts[1].draft.version,
                    candidate_id: None,
                    resolved_state: None,
                },
            ],
            title: Some("Update two files".to_owned()),
            description: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ServerError::ReconciliationRequired { draft_id, .. }
            if draft_id == drafts[1].draft.draft_id
    ));
    let after_failure = server::app::draft::get_draft(
        &pool,
        &common::owner_principal(&pool).await,
        &drafts[0].draft.draft_id,
    )
    .await
    .unwrap();
    assert_eq!(after_failure.draft.version, before.draft.version);
    assert_eq!(
        after_failure.draft.base_commit_id,
        before.draft.base_commit_id
    );

    let review = server::app::review::create_review(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        current_head.as_deref(),
        CreateReviewRequest {
            drafts: vec![
                ReviewDraftRequest {
                    draft_id: drafts[0].draft.draft_id.clone(),
                    expected_draft_version: drafts[0].draft.version,
                    candidate_id: Some(first_candidate.candidate_id),
                    resolved_state: None,
                },
                ReviewDraftRequest {
                    draft_id: drafts[1].draft.draft_id.clone(),
                    expected_draft_version: drafts[1].draft.version,
                    candidate_id: Some(second_candidate.candidate_id),
                    resolved_state: None,
                },
            ],
            title: Some("Update two files".to_owned()),
            description: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(review.drafts.len(), 2);
    assert!(review.drafts.iter().all(|draft| {
        draft.draft.base_commit_id == current_head
            && draft.draft.status == server::app::draft::dto::DraftStatus::Submitted
    }));
    let revision_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM draft_revisions WHERE draft_id = ANY($1)")
            .bind(vec![
                drafts[0].draft.draft_id.clone(),
                drafts[1].draft.draft_id.clone(),
            ])
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(revision_count, 2);
    postgres.shutdown().await;
}

async fn approve_and_merge(
    pool: &sqlx::PgPool,
    user_id: &str,
    expected_ref: Option<&str>,
    draft_id: &str,
    expected_draft_version: i64,
) {
    let review = server::app::review::create_review(
        pool,
        &common::principal(pool, user_id).await,
        expected_ref,
        CreateReviewRequest {
            drafts: vec![ReviewDraftRequest {
                draft_id: draft_id.to_owned(),
                expected_draft_version,
                candidate_id: None,
                resolved_state: None,
            }],
            title: None,
            description: None,
        },
    )
    .await
    .unwrap();
    let approved = server::app::review::create_review_decision(
        pool,
        &review.review.review_id,
        &common::principal(pool, user_id).await,
        CreateReviewDecisionRequest {
            decision: ReviewDecision::Approved,
            expected_review_version: review.review.version,
            body: None,
        },
    )
    .await
    .unwrap();
    server::app::review::create_review_merge(
        pool,
        &approved.review.review_id,
        &common::principal(pool, user_id).await,
        expected_ref,
        CreateReviewMergeRequest {
            expected_review_version: approved.review.version,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn create_request_preserves_create_update_rename_order_through_review_and_merge() {
    let postgres = common::migrated_postgres().await;
    let pool = postgres.pool.clone();
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Operation Ordering",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Create Request Ordering",
    )
    .await;
    let head = server::app::commit::get_org_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id;
    let initial_path = "context/created-in-order.md";
    let final_path = "context/created-in-final-order.md";

    let draft = server::app::draft::create_draft(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        CreateDraftRequest {
            daemon_installation_id: "daemon_create_ordering".to_owned(),
            project_id: bootstrap.project_id.clone(),
            base_commit_id: head.clone(),
            title: "Create and refine in one request".to_owned(),
            description: None,
            resource: DraftResourceRef {
                scope: ResourceScope::Org,
                id: None,
                path: Some(initial_path.to_owned()),
            },
            operations: vec![
                DraftOperationInput {
                    action: DraftOperationAction::Create,
                    resource: DraftResourceRef {
                        scope: ResourceScope::Org,
                        id: None,
                        path: Some(initial_path.to_owned()),
                    },
                    content: memory_content("# Initial"),
                    new_path: None,
                },
                DraftOperationInput {
                    action: DraftOperationAction::Update,
                    resource: DraftResourceRef {
                        scope: ResourceScope::Org,
                        id: None,
                        path: Some(initial_path.to_owned()),
                    },
                    content: memory_content("# Refined"),
                    new_path: None,
                },
                DraftOperationInput {
                    action: DraftOperationAction::Rename,
                    resource: DraftResourceRef {
                        scope: ResourceScope::Org,
                        id: None,
                        path: Some(initial_path.to_owned()),
                    },
                    content: None,
                    new_path: Some(final_path.to_owned()),
                },
            ],
        },
    )
    .await
    .unwrap();

    // Make every legacy tie-breaker disagree with request order. Stable reads
    // must continue to follow ordinal, not timestamp or generated ID.
    sqlx::query(
        "UPDATE draft_operations
         SET operation_id = CASE action
                WHEN 'create' THEN 'dop_z_create'
                WHEN 'update' THEN 'dop_m_update'
                WHEN 'rename' THEN 'dop_a_rename'
                ELSE operation_id
             END,
             created_at = '2026-08-20T00:00:00Z'
         WHERE draft_id = $1",
    )
    .bind(&draft.draft.draft_id)
    .execute(&postgres.pool)
    .await
    .unwrap();

    let stored: Vec<(String, i64)> = sqlx::query_as(
        "SELECT action, ordinal FROM draft_operations
         WHERE draft_id = $1 ORDER BY ordinal",
    )
    .bind(&draft.draft.draft_id)
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        vec![
            ("create".to_owned(), 1),
            ("update".to_owned(), 2),
            ("rename".to_owned(), 3),
        ]
    );

    let detail = server::app::draft::get_draft(
        &pool,
        &common::owner_principal(&pool).await,
        &draft.draft.draft_id,
    )
    .await
    .unwrap();
    assert_eq!(
        detail
            .operations
            .iter()
            .map(|operation| operation.input.action)
            .collect::<Vec<_>>(),
        vec![
            DraftOperationAction::Create,
            DraftOperationAction::Update,
            DraftOperationAction::Rename,
        ]
    );
    approve_and_merge(
        &pool,
        &bootstrap.user_id,
        head.as_deref(),
        &detail.draft.draft_id,
        detail.draft.version,
    )
    .await;

    let created =
        server::app::memory::list_org_memories(&pool, &common::owner_principal(&pool).await)
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|memory| memory.path == final_path)
            .expect("ordered operations should materialize the final path");
    assert_eq!(
        server::app::memory::get_org_memory(
            &pool,
            &common::owner_principal(&pool).await,
            &created.memory_id
        )
        .await
        .unwrap()
        .content,
        "# Refined"
    );
    postgres.shutdown().await;
}

#[tokio::test]
async fn batch_preserves_multiple_operations_and_their_event_versions() {
    let postgres = common::migrated_postgres().await;
    let pool = postgres.pool.clone();
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Operation Ordering",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Batch Ordering",
    )
    .await;
    let resource_id = server::app::memory::create_org_context(
        &pool,
        &common::owner_principal(&pool).await,
        "context/batch-order.md",
        "# Authority",
    )
    .await
    .unwrap();
    server::app::memory::select_org_resource_for_project(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        &resource_id,
    )
    .await
    .unwrap();
    let head = server::app::commit::get_org_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id;
    let target = DraftResourceRef {
        scope: ResourceScope::Org,
        id: Some(resource_id.clone()),
        path: None,
    };
    let draft = server::app::draft::create_draft(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        CreateDraftRequest {
            daemon_installation_id: "daemon_batch_ordering".to_owned(),
            project_id: bootstrap.project_id.clone(),
            base_commit_id: head.clone(),
            title: "Apply ordered batch".to_owned(),
            description: None,
            resource: target.clone(),
            operations: Vec::new(),
        },
    )
    .await
    .unwrap();

    let batch = server::app::draft::create_draft_operation_batch(
        &pool,
        &AuthPrincipal {
            user_id: bootstrap.user_id.clone(),
            org_id: bootstrap.org_id.clone(),
            session_id: "session_batch_ordering".to_owned(),
            token_id: "token_batch_ordering".to_owned(),
            role: "owner".to_owned(),
        },
        DraftOperationBatchRequest {
            daemon_installation_id: "daemon_batch_ordering".to_owned(),
            operations: vec![
                DraftOperationBatchItem {
                    local_operation_id: "local_first".to_owned(),
                    draft_id: draft.draft.draft_id.clone(),
                    expected_draft_version: 1,
                    operation: DraftOperationInput {
                        action: DraftOperationAction::Update,
                        resource: target.clone(),
                        content: memory_content("# First"),
                        new_path: None,
                    },
                },
                DraftOperationBatchItem {
                    local_operation_id: "local_final".to_owned(),
                    draft_id: draft.draft.draft_id.clone(),
                    expected_draft_version: 2,
                    operation: DraftOperationInput {
                        action: DraftOperationAction::Update,
                        resource: target,
                        content: memory_content("# Final"),
                        new_path: None,
                    },
                },
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(batch.accepted_operations, ["local_first", "local_final"]);

    sqlx::query(
        "UPDATE draft_operations
         SET operation_id = CASE content->>'content'
                WHEN '# First' THEN 'dop_z_batch_first'
                WHEN '# Final' THEN 'dop_a_batch_final'
                ELSE operation_id
             END,
             created_at = '2026-08-20T00:00:00Z'
         WHERE draft_id = $1",
    )
    .bind(&draft.draft.draft_id)
    .execute(&postgres.pool)
    .await
    .unwrap();

    let stored: Vec<(String, i64)> = sqlx::query_as(
        "SELECT content->>'content', ordinal
         FROM draft_operations
         WHERE draft_id = $1
         ORDER BY ordinal",
    )
    .bind(&draft.draft.draft_id)
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        vec![("# First".to_owned(), 1), ("# Final".to_owned(), 2)]
    );

    let detail = server::app::draft::get_draft(
        &pool,
        &common::owner_principal(&pool).await,
        &draft.draft.draft_id,
    )
    .await
    .unwrap();
    assert_eq!(
        detail
            .operations
            .iter()
            .map(|operation| {
                (
                    operation.input.content.as_ref().unwrap().content.as_str(),
                    operation.operation_id.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("# First", "dop_z_batch_first"),
            ("# Final", "dop_a_batch_final"),
        ]
    );
    assert_eq!(detail.draft.version, 3);

    let events = server::app::draft::list_draft_events(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        None,
        None,
    )
    .await
    .unwrap()
    .events
    .into_iter()
    .filter(|event| event.draft_id == draft.draft.draft_id)
    .map(|event| (event.event_type, event.version))
    .collect::<Vec<_>>();
    assert_eq!(
        events,
        vec![
            (server::app::draft::dto::DraftEventType::Created, 1),
            (
                server::app::draft::dto::DraftEventType::OperationAppended,
                2
            ),
            (
                server::app::draft::dto::DraftEventType::OperationAppended,
                3
            ),
        ]
    );

    approve_and_merge(
        &pool,
        &bootstrap.user_id,
        head.as_deref(),
        &detail.draft.draft_id,
        detail.draft.version,
    )
    .await;
    assert_eq!(
        server::app::memory::get_org_memory(
            &pool,
            &common::owner_principal(&pool).await,
            &resource_id
        )
        .await
        .unwrap()
        .content,
        "# Final"
    );
    postgres.shutdown().await;
}
