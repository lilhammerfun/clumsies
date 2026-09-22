//! Daemon synchronization and publication against the production server over real TCP.

use daemon::{
    DaemonConfig, DaemonContentDraftUpdate, DaemonCreateDraftOperation, DaemonDeleteDraftOperation,
    DaemonDraftContent, DaemonDraftListQuery, DaemonDraftOperation,
    DaemonDraftOperationRecordSource, DaemonDraftOperationRequest, DaemonDraftOperationSource,
    DaemonDraftResourceKind, DaemonDraftScope, DaemonIpcService, DaemonLocalDraftStatus,
    DaemonMemoryCacheRequest, DaemonMemoryCacheState, DaemonProjectCheckoutRequest,
    DaemonProjectSelectionRequest, DaemonProjectStorageMoveState,
    DaemonProjectStorageReplaceRequest, DaemonProjectStorageRequest, DaemonRenameDraftOperation,
    DaemonServerRequest, DaemonSyncRetryRequest, DaemonUpdateDraftOperation,
    DraftOperationSyncStatus, LoadMemoryRequest, SyncRetryChannel, SyncState,
};
use server::app::draft::dto::{
    CreateDraftRebaseRequest, CreateDraftRequest, DraftOperationAction, DraftOperationInput,
    DraftResourceContent, DraftResourceRef, ReconciliationCandidateStatus,
};
use server::app::memory::dto::{ReplaceProjectOrgSelectionRequest, ResourceScope};
use server::app::project::dto::{CreateProjectRequest, Project};
use server::app::review::dto::{
    CreateReviewDecisionRequest, CreateReviewMergeRequest, CreateReviewRequest,
    CreateReviewSubmissionRequest, ReviewDecision, ReviewDraftRequest, ReviewMergeResult,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

mod common;

#[cfg(target_os = "macos")]
fn directory_handoff_bookmark(path: &std::path::Path) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use objc2::runtime::Bool;
    use objc2_foundation::{
        NSData, NSString, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
    };

    let path = NSString::from_str(&path.display().to_string());
    let url = NSURL::fileURLWithPath_isDirectory(&path, true);
    let security_bookmark = url
        .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions::WithSecurityScope,
            None,
            None,
        )
        .unwrap();
    let data = NSData::with_bytes(&security_bookmark.to_vec());
    let mut stale = Bool::NO;
    let granted = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            &data,
            NSURLBookmarkResolutionOptions::WithSecurityScope,
            None,
            &mut stale,
        )
    }
    .unwrap();
    assert!(unsafe { granted.startAccessingSecurityScopedResource() });
    let handoff = granted
        .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions::empty(),
            None,
            None,
        )
        .unwrap();
    unsafe { granted.stopAccessingSecurityScopedResource() };
    STANDARD.encode(handoff.to_vec())
}

#[cfg(target_os = "macos")]
async fn wait_for_storage_move(
    state: &daemon::DaemonState,
    move_id: &str,
) -> daemon::DaemonProjectStorageMove {
    for _ in 0..100 {
        let current = state
            .project_storage_move(daemon::DaemonProjectStorageMoveRequest {
                move_id: move_id.to_owned(),
            })
            .await
            .unwrap();
        if current.state.is_terminal() {
            return current;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("Project storage move {move_id} did not finish");
}

fn context_content(content: &str) -> DaemonDraftContent {
    DaemonDraftContent {
        description: None,
        content: content.to_owned(),
    }
}

fn rule_content(content: &str) -> DaemonDraftContent {
    DaemonDraftContent {
        description: None,
        content: content.to_owned(),
    }
}

fn workflow_content(content: &str) -> DaemonDraftContent {
    DaemonDraftContent {
        description: None,
        content: content.to_owned(),
    }
}

async fn approve_and_merge(
    pool: &sqlx::PgPool,
    draft_id: &str,
    expected_draft_version: i64,
    expected_ref: Option<&str>,
) -> ReviewMergeResult {
    let detail =
        server::app::draft::get_draft(pool, &common::owner_principal(pool).await, draft_id)
            .await
            .unwrap();
    let review = server::app::review::create_review(
        pool,
        &common::principal(pool, &detail.draft.author.user_id).await,
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
        &common::principal(pool, &review.review.author.user_id).await,
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
        &common::principal(pool, &approved.review.author.user_id).await,
        expected_ref,
        CreateReviewMergeRequest {
            expected_review_version: approved.review.version,
        },
    )
    .await
    .unwrap()
}

async fn sync_local_draft_and_merge(
    service: &DaemonIpcService,
    pool: &sqlx::PgPool,
    local_draft_id: &str,
    expected_ref: Option<&str>,
) -> String {
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let projection = service.get_draft(local_draft_id).await.unwrap();
    let merge = approve_and_merge(
        pool,
        projection.draft.server_draft_id.as_deref().unwrap(),
        projection.draft.server_version,
        expected_ref,
    )
    .await;
    let commit_id = merge.commit_id.unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::All,
        })
        .await
        .unwrap();
    assert_eq!(
        service
            .get_draft(local_draft_id)
            .await
            .unwrap()
            .draft
            .status,
        DaemonLocalDraftStatus::Merged
    );
    commit_id
}

async fn create_resource_draft(
    service: &DaemonIpcService,
    project_id: &str,
    scope: DaemonDraftScope,
    resource: DaemonDraftResourceKind,
    path: &str,
    content: DaemonDraftContent,
    source: DaemonDraftOperationSource,
) -> String {
    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: project_id.to_owned(),
            scope,
            resource,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: path.to_owned(),
                    content,
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(source),
        })
        .await
        .unwrap()
        .draft_id
}

async fn update_and_rename_resource_draft(
    service: &DaemonIpcService,
    project_id: &str,
    target: (DaemonDraftScope, DaemonDraftResourceKind),
    resource_id: &str,
    content: DaemonDraftContent,
    new_path: &str,
    update_source: DaemonDraftOperationSource,
) -> String {
    let (scope, resource) = target;
    let draft_id = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: project_id.to_owned(),
            scope,
            resource,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: resource_id.to_owned(),
                        content,
                        description: None,
                    },
                )),
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(update_source),
        })
        .await
        .unwrap()
        .draft_id;
    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(draft_id.clone()),
            base_commit_id: None,
            project_id: project_id.to_owned(),
            scope,
            resource,
            op: DaemonDraftOperation {
                create: None,
                update: None,
                rename: Some(DaemonRenameDraftOperation {
                    id: resource_id.to_owned(),
                    new_path: new_path.to_owned(),
                    description: None,
                }),
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();
    draft_id
}

async fn delete_resource_draft(
    service: &DaemonIpcService,
    project_id: &str,
    scope: DaemonDraftScope,
    resource: DaemonDraftResourceKind,
    resource_id: &str,
) -> String {
    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: project_id.to_owned(),
            scope,
            resource,
            op: DaemonDraftOperation {
                create: None,
                update: None,
                rename: None,
                delete: Some(DaemonDeleteDraftOperation {
                    id: resource_id.to_owned(),
                    description: None,
                }),
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap()
        .draft_id
}

async fn cache_root_for_commit(
    service: &DaemonIpcService,
    project_id: &str,
    commit_id: &str,
) -> std::path::PathBuf {
    let cache = service
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: project_id.to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(cache.commit_id.as_deref(), Some(commit_id));
    std::path::PathBuf::from(cache.active_generation_path.unwrap())
}

async fn current_project_commit_id(pool: &sqlx::PgPool, project_id: &str) -> String {
    server::app::commit::get_project_commit_state(
        pool,
        &common::owner_principal(pool).await,
        project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap()
}

#[tokio::test]
async fn project_creation_proxy_preserves_idempotency_and_replays_the_result() {
    let postgres = common::start_postgres().await;
    let database_url = &postgres.database_url;
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    server::infra::database::run_migrations(&pool)
        .await
        .unwrap();
    let bootstrap = common::initialize_installation(pool.clone(), "Project Proxy").await;

    let access_token = "daemon-project-create-access-token";
    let token_hash = hex::encode(Sha256::digest(access_token.as_bytes()));
    sqlx::query(
        "INSERT INTO auth_sessions (session_id, user_id, org_id)
         VALUES ('ses_daemon_project_create', $1, $2)",
    )
    .bind(&bootstrap.user_id)
    .bind(&bootstrap.org_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_tokens (
            token_id, session_id, user_id, kind, token_hash, expires_at
         ) VALUES (
            'tok_daemon_project_create', 'ses_daemon_project_create', $1,
            'access', $2, now() + interval '30 minutes'
         )",
    )
    .bind(&bootstrap.user_id)
    .bind(token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let server = common::TestServer::start(pool.clone(), postgres).await;
    let server_address = server.address;

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{server_address}");
    config.project.project_id = Some(bootstrap.project_id);
    let (state, _) = common::initialize_authenticated_daemon(config, access_token, None).await;
    let request = DaemonServerRequest {
        method: "POST".to_owned(),
        path: "/api/v1/projects".to_owned(),
        headers: BTreeMap::from([
            ("Content-Type".to_owned(), "application/json".to_owned()),
            (
                "Idempotency-Key".to_owned(),
                "daemon-project-create-1".to_owned(),
            ),
        ]),
        body: Some(
            serde_json::to_string(&CreateProjectRequest {
                name: "Created Through Daemon".to_owned(),
                description: Some("Proxy contract verification".to_owned()),
            })
            .unwrap(),
        ),
    };

    let first = state.server_request(request.clone()).await.unwrap();
    assert_eq!(first.status, 201);
    let first_project: Project = serde_json::from_str(&first.body).unwrap();

    let replay = state.server_request(request).await.unwrap();
    assert_eq!(replay.status, 201);
    let replayed_project: Project = serde_json::from_str(&replay.body).unwrap();
    assert_eq!(replayed_project.project_id, first_project.project_id);

    let project_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE lower(name) = lower($1)")
            .bind("Created Through Daemon")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(project_count, 1);

    server.shutdown().await;
}

#[tokio::test]
async fn local_draft_refreshes_auth_and_syncs_to_the_real_server() {
    let postgres = common::start_postgres().await;
    let database_url = &postgres.database_url;
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    server::infra::database::run_migrations(&pool)
        .await
        .unwrap();
    let bootstrap = common::initialize_installation(pool.clone(), "Daemon Integration").await;

    let stale_access_token = "expired-daemon-access-token";
    let refresh_token = "daemon-integration-refresh-token";
    let token_hash = hex::encode(Sha256::digest(refresh_token.as_bytes()));
    sqlx::query(
        "INSERT INTO auth_sessions (session_id, user_id, org_id)
         VALUES ('ses_daemon_integration', $1, $2)",
    )
    .bind(&bootstrap.user_id)
    .bind(&bootstrap.org_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO access_tokens (
            token_id, session_id, user_id, kind, token_hash, expires_at
         ) VALUES (
            'tok_daemon_refresh', 'ses_daemon_integration', $1,
            'refresh', $2, now() + interval '30 days'
         )",
    )
    .bind(&bootstrap.user_id)
    .bind(token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let server = common::TestServer::start(pool.clone(), postgres).await;
    let server_address = server.address;
    let verification_pool = pool.clone();

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{server_address}");
    config.project.project_id = Some(bootstrap.project_id.clone());
    let (state, credential_store) = common::initialize_authenticated_daemon(
        config,
        stale_access_token,
        Some(refresh_token.to_owned()),
    )
    .await;
    let service = DaemonIpcService::new(state);

    let local_draft = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/from-daemon.md".to_owned(),
                    content: context_content("# Synced\n\nCreated through the local daemon."),
                    description: Some("Real Server integration".to_owned()),
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::McpStore),
        })
        .await
        .unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    let sync = service.sync_status().await.unwrap();
    assert_eq!(sync.pending_operation_count, 0);
    assert_eq!(sync.failed_operation_count, 0);
    assert!(sync.draft_sync.server_cursor.is_some());
    let project_config = service.project_config();
    assert!(project_config.has_access_token);
    assert!(project_config.has_refresh_token);
    let refreshed_credentials = credential_store.credentials().unwrap();
    assert_ne!(refreshed_credentials.access_token, stale_access_token);
    assert_ne!(
        refreshed_credentials.refresh_token.as_deref(),
        Some(refresh_token)
    );

    let active_token_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM access_tokens
         WHERE session_id = 'ses_daemon_integration' AND revoked_at IS NULL",
    )
    .fetch_one(&verification_pool)
    .await
    .unwrap();
    assert_eq!(active_token_count, 2);

    let drafts = server::app::draft::list_drafts(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        Some(&bootstrap.project_id),
    )
    .await
    .unwrap();
    assert_eq!(drafts.items.len(), 1);
    assert_eq!(drafts.items[0].author.user_id, bootstrap.user_id);
    let draft = server::app::draft::get_draft(
        &pool,
        &common::owner_principal(&pool).await,
        &drafts.items[0].draft_id,
    )
    .await
    .unwrap();
    assert_eq!(draft.operations.len(), 1);
    assert_eq!(
        draft.operations[0].input.content,
        Some(DraftResourceContent {
            description: None,
            content: "# Synced\n\nCreated through the local daemon.".to_owned(),
        })
    );

    let review = server::app::review::create_review(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        None,
        CreateReviewRequest {
            drafts: vec![ReviewDraftRequest {
                draft_id: draft.draft.draft_id.clone(),
                expected_draft_version: draft.draft.version,
                candidate_id: None,
                resolved_state: None,
            }],
            title: None,
            description: None,
        },
    )
    .await
    .unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let projected = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(projected.draft.status, DaemonLocalDraftStatus::Submitted);
    assert_eq!(projected.draft.server_version, review.draft.version);

    let rejected = server::app::review::create_review_decision(
        &pool,
        &review.review.review_id,
        &common::principal(&pool, &review.review.author.user_id).await,
        CreateReviewDecisionRequest {
            decision: ReviewDecision::Rejected,
            expected_review_version: review.review.version,
            body: Some("Revise the context.".to_owned()),
        },
    )
    .await
    .unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let projected = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(projected.draft.status, DaemonLocalDraftStatus::Open);
    assert_eq!(projected.draft.server_version, rejected.draft.version);

    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(local_draft.draft_id.clone()),
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/from-daemon.md".to_owned(),
                    content: context_content("# Revised\n\nEdited after the rejected review."),
                    description: Some("Rejected review revision".to_owned()),
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let edited = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(edited.draft.status, DaemonLocalDraftStatus::Open);
    assert_eq!(edited.draft.server_version, rejected.draft.version + 1);

    let resubmitted = server::app::review::create_review_submission(
        &pool,
        &rejected.review.review_id,
        &common::principal(&pool, &bootstrap.user_id).await,
        rejected.draft.coordination.current_commit_id.as_deref(),
        CreateReviewSubmissionRequest {
            expected_review_version: rejected.review.version,
            drafts: vec![ReviewDraftRequest {
                draft_id: rejected.draft.draft_id.clone(),
                expected_draft_version: edited.draft.server_version,
                candidate_id: None,
                resolved_state: None,
            }],
            title: Some("Revised daemon context".to_owned()),
            description: None,
        },
    )
    .await
    .unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let projected = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(projected.draft.status, DaemonLocalDraftStatus::Submitted);
    assert_eq!(projected.draft.server_version, resubmitted.draft.version);
    assert_eq!(resubmitted.review.review_id, review.review.review_id);

    server.shutdown().await;
}

#[tokio::test]
async fn merged_context_drafts_are_terminal_across_update_rename_and_delete() {
    let postgres = common::start_postgres().await;
    let database_url = &postgres.database_url;
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    server::infra::database::run_migrations(&pool)
        .await
        .unwrap();
    let bootstrap = common::initialize_installation(pool.clone(), "Context Lifecycle").await;

    let seed_draft = server::app::draft::create_draft(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        CreateDraftRequest {
            daemon_installation_id: "daemon_seed_context".to_owned(),
            project_id: bootstrap.project_id.clone(),
            base_commit_id: None,
            title: "Seed context".to_owned(),
            description: None,
            resource: DraftResourceRef {
                scope: ResourceScope::Org,
                id: None,
                path: Some("context/original.md".to_owned()),
            },
            operations: vec![DraftOperationInput {
                action: DraftOperationAction::Create,
                resource: DraftResourceRef {
                    scope: ResourceScope::Org,
                    id: None,
                    path: Some("context/original.md".to_owned()),
                },
                content: Some(DraftResourceContent {
                    description: None,
                    content: "# Original\n\nBefore local editing.".to_owned(),
                }),
                new_path: None,
            }],
        },
    )
    .await
    .unwrap();
    let seed_merge = approve_and_merge(
        &pool,
        &seed_draft.draft.draft_id,
        seed_draft.draft.version,
        None,
    )
    .await;
    let seed_org_commit_id = seed_merge.commit_id.unwrap();
    let context_id =
        server::app::memory::list_org_memories(&pool, &common::owner_principal(&pool).await)
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|memory| memory.path == "context/original.md")
            .unwrap()
            .memory_id;

    let access_token = "daemon-context-lifecycle-access-token";
    let token_hash = hex::encode(Sha256::digest(access_token.as_bytes()));
    sqlx::query(
        "INSERT INTO auth_sessions (session_id, user_id, org_id)
         VALUES ('ses_daemon_context_lifecycle', $1, $2)",
    )
    .bind(&bootstrap.user_id)
    .bind(&bootstrap.org_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_tokens (
            token_id, session_id, user_id, kind, token_hash, expires_at
         ) VALUES (
            'tok_daemon_context_lifecycle', 'ses_daemon_context_lifecycle', $1,
            'access', $2, now() + interval '30 minutes'
         )",
    )
    .bind(&bootstrap.user_id)
    .bind(token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let server = common::TestServer::start(pool.clone(), postgres).await;
    let server_address = server.address;

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{server_address}");
    config.project.project_id = Some(bootstrap.project_id.clone());
    let (state, _) = common::initialize_authenticated_daemon(config, access_token, None).await;
    #[cfg(target_os = "macos")]
    let custom_storage = {
        let custom_storage = tempfile::tempdir().unwrap();
        let initial = state
            .project_storage(DaemonProjectStorageRequest {
                project_id: bootstrap.project_id.clone(),
            })
            .await
            .unwrap();
        let storage_move = state
            .replace_project_storage(DaemonProjectStorageReplaceRequest {
                project_id: bootstrap.project_id.clone(),
                selected_root_path: custom_storage.path().display().to_string(),
                handoff_bookmark_data: directory_handoff_bookmark(custom_storage.path()),
                expected_location_revision: initial.location_revision,
            })
            .await
            .unwrap();
        let completed = wait_for_storage_move(&state, &storage_move.move_id).await;
        assert_eq!(
            completed.state,
            DaemonProjectStorageMoveState::Completed,
            "storage move failed: {:?}",
            completed.error_message
        );
        custom_storage
    };
    let service = DaemonIpcService::new(state);
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap();

    let update_draft = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: context_id.clone(),
                        content: context_content("# Renamed\n\nUpdated through the daemon."),
                        description: None,
                    },
                )),
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();
    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(update_draft.draft_id.clone()),
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: None,
                rename: Some(DaemonRenameDraftOperation {
                    id: context_id.clone(),
                    new_path: "context/renamed.md".to_owned(),
                    description: None,
                }),
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let update_projection = service.get_draft(&update_draft.draft_id).await.unwrap();
    let update_merge = approve_and_merge(
        &pool,
        update_projection.draft.server_draft_id.as_deref().unwrap(),
        update_projection.draft.server_version,
        Some(&seed_org_commit_id),
    )
    .await;
    let update_org_commit_id = update_merge.commit_id.unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::All,
        })
        .await
        .unwrap();
    let merged_update = service.get_draft(&update_draft.draft_id).await.unwrap();
    assert_eq!(merged_update.draft.status, DaemonLocalDraftStatus::Merged);
    let update_project_commit_id = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    assert_ne!(update_project_commit_id, update_org_commit_id);
    let cache = service
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: bootstrap.project_id.clone(),
        })
        .await
        .unwrap();
    let cache_root = std::path::PathBuf::from(cache.active_generation_path.unwrap());
    #[cfg(target_os = "macos")]
    assert!(cache_root.starts_with(std::fs::canonicalize(custom_storage.path()).unwrap()));
    assert!(!cache_root.join("cache/memory/context/original.md").exists());
    assert_eq!(
        std::fs::read_to_string(cache_root.join("cache/memory/context/renamed.md")).unwrap(),
        "# Renamed\n\nUpdated through the daemon."
    );
    let loaded = service
        .load_memory(LoadMemoryRequest {
            project_id: bootstrap.project_id.clone(),
            ids: vec!["context/renamed.md".to_owned()],
            known_hashes: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(
        loaded.resources[0].content.as_deref(),
        Some("# Renamed\n\nUpdated through the daemon.")
    );

    let delete_draft = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: None,
                rename: None,
                delete: Some(DaemonDeleteDraftOperation {
                    id: context_id.clone(),
                    description: None,
                }),
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();
    assert_ne!(delete_draft.draft_id, update_draft.draft_id);
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let delete_projection = service.get_draft(&delete_draft.draft_id).await.unwrap();
    let delete_merge = approve_and_merge(
        &pool,
        delete_projection.draft.server_draft_id.as_deref().unwrap(),
        delete_projection.draft.server_version,
        Some(&update_org_commit_id),
    )
    .await;
    let delete_org_commit_id = delete_merge.commit_id.unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::All,
        })
        .await
        .unwrap();
    let merged_delete = service.get_draft(&delete_draft.draft_id).await.unwrap();
    assert_eq!(merged_delete.draft.status, DaemonLocalDraftStatus::Merged);
    let delete_project_commit_id = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    assert_ne!(delete_project_commit_id, delete_org_commit_id);
    let deleted_cache = service
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: bootstrap.project_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        deleted_cache.commit_id.as_deref(),
        Some(delete_project_commit_id.as_str())
    );
    let deleted_cache_root =
        std::path::PathBuf::from(deleted_cache.active_generation_path.unwrap());
    assert_ne!(deleted_cache_root, cache_root);
    assert!(cache_root.join("cache/memory/context/renamed.md").exists());
    assert!(
        !deleted_cache_root
            .join("cache/memory/context/renamed.md")
            .exists()
    );
    assert!(matches!(
        server::app::memory::get_org_memory(
            &pool,
            &common::owner_principal(&pool).await,
            &context_id
        )
        .await,
        Err(server::error::ServerError::NotFound { .. })
    ));

    server.shutdown().await;
}

#[tokio::test]
async fn offline_behind_draft_stays_editable_until_explicit_reconciliation() {
    let postgres = common::start_postgres().await;
    let database_url = &postgres.database_url;
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    server::infra::database::run_migrations(&pool)
        .await
        .unwrap();
    let verification_pool = pool.clone();
    let bootstrap = common::initialize_installation(pool.clone(), "Offline Conflict").await;

    let base_draft = server::app::draft::create_draft(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        CreateDraftRequest {
            daemon_installation_id: "daemon_conflict_seed".to_owned(),
            project_id: bootstrap.project_id.clone(),
            base_commit_id: None,
            title: "Seed conflict base".to_owned(),
            description: None,
            resource: DraftResourceRef {
                scope: ResourceScope::Org,
                id: None,
                path: Some("context/base.md".to_owned()),
            },
            operations: vec![DraftOperationInput {
                action: DraftOperationAction::Create,
                resource: DraftResourceRef {
                    scope: ResourceScope::Org,
                    id: None,
                    path: Some("context/base.md".to_owned()),
                },
                content: Some(DraftResourceContent {
                    description: None,
                    content: "# Base\n\nThe offline Draft starts from this Commit.".to_owned(),
                }),
                new_path: None,
            }],
        },
    )
    .await
    .unwrap();
    let base_merge = approve_and_merge(
        &pool,
        &base_draft.draft.draft_id,
        base_draft.draft.version,
        None,
    )
    .await;
    let base_commit_id = base_merge.commit_id.unwrap();

    let access_token = "daemon-offline-conflict-access-token";
    let token_hash = hex::encode(Sha256::digest(access_token.as_bytes()));
    sqlx::query(
        "INSERT INTO auth_sessions (session_id, user_id, org_id)
         VALUES ('ses_daemon_offline_conflict', $1, $2)",
    )
    .bind(&bootstrap.user_id)
    .bind(&bootstrap.org_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_tokens (
            token_id, session_id, user_id, kind, token_hash, expires_at
         ) VALUES (
            'tok_daemon_offline_conflict', 'ses_daemon_offline_conflict', $1,
            'access', $2, now() + interval '30 minutes'
         )",
    )
    .bind(&bootstrap.user_id)
    .bind(token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let server = common::TestServer::start(pool.clone(), postgres).await;
    let server_address = server.address;
    let server_url = format!("http://{server_address}");
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = server_url.clone();
    config.project.project_id = Some(bootstrap.project_id.clone());
    let (state, _) = common::initialize_authenticated_daemon(config, access_token, None).await;
    let service = DaemonIpcService::new(state.clone());
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap();

    service
        .replace_project_config(daemon::DaemonProjectConfigUpdateRequest {
            server_url: "http://127.0.0.1:1".to_owned(),
            project_id: Some(bootstrap.project_id.clone()),
            access_token: Some(access_token.to_owned()),
            refresh_token: None,
            memory_guidelines_path: None,
        })
        .await
        .unwrap();
    let local_draft = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/offline-conflict.md".to_owned(),
                    content: context_content(
                        "# Offline conflict\n\nThis content must survive recovery and resolution.",
                    ),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();
    assert!(
        service
            .retry_sync(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::Drafts,
            })
            .await
            .is_err()
    );
    // Identity preflight failed before any upload; the saved operation remains queued.
    let retrying = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(
        retrying.draft.base_commit_id.as_deref(),
        Some(base_commit_id.as_str())
    );
    assert_eq!(
        retrying.operations[0].sync_status,
        DraftOperationSyncStatus::Queued
    );

    let remote_draft = server::app::draft::create_draft(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        CreateDraftRequest {
            daemon_installation_id: "daemon_remote_change".to_owned(),
            project_id: bootstrap.project_id.clone(),
            base_commit_id: Some(base_commit_id.clone()),
            title: "Advance the remote Ref".to_owned(),
            description: None,
            resource: DraftResourceRef {
                scope: ResourceScope::Org,
                id: None,
                path: Some("context/remote-change.md".to_owned()),
            },
            operations: vec![DraftOperationInput {
                action: DraftOperationAction::Create,
                resource: DraftResourceRef {
                    scope: ResourceScope::Org,
                    id: None,
                    path: Some("context/remote-change.md".to_owned()),
                },
                content: Some(DraftResourceContent {
                    description: None,
                    content: "# Remote change\n\nThis advances the Project Ref.".to_owned(),
                }),
                new_path: None,
            }],
        },
    )
    .await
    .unwrap();
    let remote_merge = approve_and_merge(
        &pool,
        &remote_draft.draft.draft_id,
        remote_draft.draft.version,
        Some(&base_commit_id),
    )
    .await;
    let current_commit_id = remote_merge.commit_id.unwrap();

    let local_pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    for statement in [
        "DELETE FROM cached_commits",
        "DELETE FROM cached_trees",
        "DELETE FROM cached_blobs",
    ] {
        sqlx::query(statement).execute(&local_pool).await.unwrap();
    }

    service
        .replace_project_config(daemon::DaemonProjectConfigUpdateRequest {
            server_url: server_url.clone(),
            project_id: Some(bootstrap.project_id.clone()),
            access_token: Some(access_token.to_owned()),
            refresh_token: None,
            memory_guidelines_path: None,
        })
        .await
        .unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap();
    let retained_base_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cached_commits WHERE commit_id = $1")
            .bind(&base_commit_id)
            .fetch_one(&local_pool)
            .await
            .unwrap();
    assert_eq!(retained_base_count, 1);
    let effective = service
        .load_memory(LoadMemoryRequest {
            project_id: bootstrap.project_id.clone(),
            ids: vec![
                "context/offline-conflict.md".to_owned(),
                "context/remote-change.md".to_owned(),
            ],
            known_hashes: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(effective.resources.len(), 2);
    assert_eq!(
        effective
            .resources
            .iter()
            .find(|resource| resource.path == "context/offline-conflict.md")
            .and_then(|resource| resource.content.as_deref()),
        Some("# Offline conflict\n\nThis content must survive recovery and resolution.")
    );
    assert!(
        effective
            .resources
            .iter()
            .any(|resource| resource.path == "context/remote-change.md")
    );
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let uploaded = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(uploaded.draft.status, DaemonLocalDraftStatus::Open);
    assert_eq!(
        uploaded.draft.base_commit_id.as_deref(),
        Some(base_commit_id.as_str())
    );
    assert_eq!(uploaded.operations.len(), 1);
    assert_eq!(
        uploaded.operations[0].sync_status,
        DraftOperationSyncStatus::Synced
    );

    service
        .replace_project_config(daemon::DaemonProjectConfigUpdateRequest {
            server_url: "http://127.0.0.1:1".to_owned(),
            project_id: Some(bootstrap.project_id.clone()),
            access_token: Some(access_token.to_owned()),
            refresh_token: None,
            memory_guidelines_path: None,
        })
        .await
        .unwrap();
    let later_offline_content =
        "# Offline conflict\n\nThis later offline edit must be included in the resolution.";
    let later_offline_operation = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(local_draft.draft_id.clone()),
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/offline-conflict.md".to_owned(),
                    content: context_content(later_offline_content),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();
    assert!(
        service
            .retry_sync(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::Drafts,
            })
            .await
            .is_err()
    );
    let later_offline = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(later_offline.operations.len(), 2);
    assert_eq!(
        later_offline
            .operations
            .iter()
            .find(|operation| {
                operation.local_operation_id == later_offline_operation.local_operation_id
            })
            .unwrap()
            .sync_status,
        DraftOperationSyncStatus::Queued
    );

    service
        .replace_project_config(daemon::DaemonProjectConfigUpdateRequest {
            server_url,
            project_id: Some(bootstrap.project_id.clone()),
            access_token: Some(access_token.to_owned()),
            refresh_token: None,
            memory_guidelines_path: None,
        })
        .await
        .unwrap();
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let projected_behind = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(projected_behind.draft.status, DaemonLocalDraftStatus::Open);
    assert_eq!(
        projected_behind.draft.base_commit_id.as_deref(),
        Some(base_commit_id.as_str())
    );
    assert_eq!(
        projected_behind.draft.current_commit_id.as_deref(),
        Some(current_commit_id.as_str())
    );
    assert_eq!(
        projected_behind.draft.freshness,
        daemon::DaemonDraftFreshness::Behind
    );
    assert!(!projected_behind.draft.has_upstream_resource_changes);
    let synced_offline_operation = projected_behind
        .operations
        .iter()
        .find(|operation| {
            operation.local_operation_id == later_offline_operation.local_operation_id
        })
        .unwrap();
    assert_eq!(
        synced_offline_operation.sync_status,
        DraftOperationSyncStatus::Synced
    );
    assert_eq!(
        synced_offline_operation
            .operation
            .create
            .as_ref()
            .unwrap()
            .content,
        context_content(later_offline_content)
    );
    assert_eq!(
        service.sync_status().await.unwrap().draft_sync.state,
        SyncState::Idle
    );

    let server_behind = server::app::draft::get_draft(
        &pool,
        &common::owner_principal(&pool).await,
        projected_behind.draft.server_draft_id.as_deref().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        server_behind.draft.base_commit_id,
        Some(base_commit_id.clone())
    );
    assert_eq!(server_behind.operations.len(), 2);

    let reconciliation_error = server::app::review::create_review(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        Some(&current_commit_id),
        CreateReviewRequest {
            drafts: vec![ReviewDraftRequest {
                draft_id: server_behind.draft.draft_id.clone(),
                expected_draft_version: server_behind.draft.version,
                candidate_id: None,
                resolved_state: None,
            }],
            title: None,
            description: None,
        },
    )
    .await
    .unwrap_err();
    let candidate_id = match reconciliation_error {
        server::error::ServerError::ReconciliationRequired { candidate_id, .. } => candidate_id,
        error => panic!("expected reconciliation_required, got {error:?}"),
    };
    let candidate = server::app::draft::get_draft_reconciliation_candidate(
        &pool,
        &common::owner_principal(&pool).await,
        &server_behind.draft.draft_id,
        &candidate_id,
    )
    .await
    .unwrap();
    assert_eq!(candidate.status, ReconciliationCandidateStatus::Clean);
    assert!(candidate.valid);
    assert_eq!(candidate.base_commit_id, Some(base_commit_id.clone()));
    assert_eq!(candidate.current_commit_id, Some(current_commit_id.clone()));

    let unchanged = server::app::draft::get_draft(
        &pool,
        &common::owner_principal(&pool).await,
        &server_behind.draft.draft_id,
    )
    .await
    .unwrap();
    assert_eq!(unchanged.draft.base_commit_id, Some(base_commit_id.clone()));
    assert_eq!(unchanged.draft.version, server_behind.draft.version);
    assert_eq!(unchanged.operations, server_behind.operations);

    let rebased = server::app::draft::create_draft_rebase(
        &pool,
        &server_behind.draft.draft_id,
        &common::principal(&pool, &bootstrap.user_id).await,
        Some(&current_commit_id),
        CreateDraftRebaseRequest {
            candidate_id,
            expected_draft_version: server_behind.draft.version,
            resolved_state: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        rebased.draft.draft.base_commit_id.as_deref(),
        Some(current_commit_id.as_str())
    );
    assert_eq!(rebased.draft.operations.len(), 1);
    let revision_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM draft_revisions WHERE draft_id = $1")
            .bind(&server_behind.draft.draft_id)
            .fetch_one(&verification_pool)
            .await
            .unwrap();
    assert_eq!(revision_count, 1);

    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let projected_resolution = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(
        projected_resolution.draft.status,
        DaemonLocalDraftStatus::Open
    );
    assert_eq!(
        projected_resolution.draft.base_commit_id.as_deref(),
        Some(current_commit_id.as_str())
    );
    assert_eq!(
        projected_resolution.draft.current_commit_id,
        Some(current_commit_id.clone())
    );
    assert_eq!(
        projected_resolution.draft.freshness,
        daemon::DaemonDraftFreshness::Current
    );
    assert_eq!(projected_resolution.operations.len(), 1);
    assert_eq!(
        projected_resolution.operations[0].sync_status,
        DraftOperationSyncStatus::Synced
    );
    assert!(projected_resolution.operations[0].last_error.is_none());
    assert_eq!(
        projected_resolution.operations[0]
            .operation
            .create
            .as_ref()
            .unwrap()
            .content,
        context_content(later_offline_content)
    );
    let resolved_sync_status = service.sync_status().await.unwrap();
    assert_eq!(resolved_sync_status.failed_operation_count, 0);
    assert_eq!(resolved_sync_status.draft_sync.state, SyncState::Idle);

    let review = server::app::review::create_review(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        Some(&current_commit_id),
        CreateReviewRequest {
            drafts: vec![ReviewDraftRequest {
                draft_id: rebased.draft.draft.draft_id.clone(),
                expected_draft_version: rebased.draft.draft.version,
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
        &pool,
        &review.review.review_id,
        &common::principal(&pool, &review.review.author.user_id).await,
        CreateReviewDecisionRequest {
            decision: ReviewDecision::Approved,
            expected_review_version: review.review.version,
            body: Some("Reviewed against the current Ref.".to_owned()),
        },
    )
    .await
    .unwrap();
    let final_merge = server::app::review::create_review_merge(
        &pool,
        &approved.review.review_id,
        &common::principal(&pool, &approved.review.author.user_id).await,
        Some(&current_commit_id),
        CreateReviewMergeRequest {
            expected_review_version: approved.review.version,
        },
    )
    .await
    .unwrap();
    let final_commit_id = final_merge.commit_id.unwrap();
    assert_ne!(final_commit_id, current_commit_id);

    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::All,
        })
        .await
        .unwrap();
    let merged = service.get_draft(&local_draft.draft_id).await.unwrap();
    assert_eq!(merged.draft.status, DaemonLocalDraftStatus::Merged);
    let final_project_commit_id = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    assert_ne!(final_project_commit_id, final_commit_id);
    let cache = service
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: bootstrap.project_id,
        })
        .await
        .unwrap();
    assert_eq!(
        cache.commit_id.as_deref(),
        Some(final_project_commit_id.as_str())
    );
    let cache_root = std::path::PathBuf::from(cache.active_generation_path.unwrap());
    assert_eq!(
        std::fs::read_to_string(cache_root.join("cache/memory/context/offline-conflict.md"))
            .unwrap(),
        later_offline_content
    );
    assert!(
        cache_root
            .join("cache/memory/context/remote-change.md")
            .is_file()
    );

    server.shutdown().await;
}

#[tokio::test]
async fn rule_and_workflow_crud_preserve_materialized_markdown() {
    let postgres = common::start_postgres().await;
    let database_url = &postgres.database_url;
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    server::infra::database::run_migrations(&pool)
        .await
        .unwrap();
    let bootstrap = common::initialize_installation(pool.clone(), "Structured Lifecycle").await;

    let access_token = "daemon-structured-lifecycle-access-token";
    let token_hash = hex::encode(Sha256::digest(access_token.as_bytes()));
    sqlx::query(
        "INSERT INTO auth_sessions (session_id, user_id, org_id)
         VALUES ('ses_daemon_structured_lifecycle', $1, $2)",
    )
    .bind(&bootstrap.user_id)
    .bind(&bootstrap.org_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_tokens (
            token_id, session_id, user_id, kind, token_hash, expires_at
         ) VALUES (
            'tok_daemon_structured_lifecycle', 'ses_daemon_structured_lifecycle', $1,
            'access', $2, now() + interval '30 minutes'
         )",
    )
    .bind(&bootstrap.user_id)
    .bind(token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let server = common::TestServer::start(pool.clone(), postgres).await;
    let server_address = server.address;

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{server_address}");
    config.project.project_id = Some(bootstrap.project_id.clone());
    let (state, _) = common::initialize_authenticated_daemon(config, access_token, None).await;
    let service = DaemonIpcService::new(state);

    let create_rule = create_resource_draft(
        &service,
        &bootstrap.project_id,
        DaemonDraftScope::Org,
        DaemonDraftResourceKind::Memory,
        "rules/memory-review",
        rule_content(
            "# Memory review discipline\n\nApply when publishing durable memory.\n\nReview every memory change before merge.\n\nTags: memory, review",
        ),
        DaemonDraftOperationSource::Desktop,
    )
    .await;
    let rule_create_org_commit =
        sync_local_draft_and_merge(&service, &pool, &create_rule, None).await;
    let rule_meta =
        server::app::memory::list_org_memories(&pool, &common::owner_principal(&pool).await)
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|memory| memory.path == "rules/memory-review")
            .unwrap();
    let rule_id = rule_meta.memory_id;
    let created_rule =
        server::app::memory::get_org_memory(&pool, &common::owner_principal(&pool).await, &rule_id)
            .await
            .unwrap();
    assert_eq!(created_rule.memory.name, "memory-review");
    assert_eq!(
        created_rule.content,
        "# Memory review discipline\n\nApply when publishing durable memory.\n\nReview every memory change before merge.\n\nTags: memory, review"
    );
    let rule_create_project_commit = current_project_commit_id(&pool, &bootstrap.project_id).await;
    assert_ne!(rule_create_project_commit, rule_create_org_commit);
    let rule_create_root =
        cache_root_for_commit(&service, &bootstrap.project_id, &rule_create_project_commit).await;
    assert_eq!(
        std::fs::read_to_string(rule_create_root.join("cache/memory/rules/memory-review")).unwrap(),
        created_rule.content
    );

    let create_workflow = create_resource_draft(
        &service,
        &bootstrap.project_id,
        DaemonDraftScope::Org,
        DaemonDraftResourceKind::Memory,
        "workflow/memory-publication",
        workflow_content(&format!(
            "# Memory publication\n\nPublish durable memory safely.\n\n1. Apply rule `{rule_id}`.\n2. Verify the materialized generation."
        )),
        DaemonDraftOperationSource::Desktop,
    )
    .await;
    let workflow_create_org_commit = sync_local_draft_and_merge(
        &service,
        &pool,
        &create_workflow,
        Some(&rule_create_org_commit),
    )
    .await;
    let workflow_meta =
        server::app::memory::list_org_memories(&pool, &common::owner_principal(&pool).await)
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|memory| memory.path == "workflow/memory-publication")
            .unwrap();
    let workflow_id = workflow_meta.memory_id;
    let created_workflow = server::app::memory::get_org_memory(
        &pool,
        &common::owner_principal(&pool).await,
        &workflow_id,
    )
    .await
    .unwrap();
    assert_eq!(
        created_workflow.content,
        format!(
            "# Memory publication\n\nPublish durable memory safely.\n\n1. Apply rule `{rule_id}`.\n2. Verify the materialized generation."
        )
    );
    let workflow_create_project_commit =
        current_project_commit_id(&pool, &bootstrap.project_id).await;
    assert_ne!(workflow_create_project_commit, workflow_create_org_commit);
    let workflow_create_root = cache_root_for_commit(
        &service,
        &bootstrap.project_id,
        &workflow_create_project_commit,
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(
            workflow_create_root.join("cache/memory/workflow/memory-publication")
        )
        .unwrap(),
        format!(
            "# Memory publication\n\nPublish durable memory safely.\n\n1. Apply rule `{rule_id}`.\n2. Verify the materialized generation."
        )
    );

    let update_rule = update_and_rename_resource_draft(
        &service,
        &bootstrap.project_id,
        (DaemonDraftScope::Org, DaemonDraftResourceKind::Memory),
        &rule_id,
        rule_content(
            "# Memory review discipline\n\nApply when publishing durable memory.\n\nReview the change and its materialized result before merge.\n\nTags: memory, review, verification",
        ),
        "rules/memory-review-policy",
        DaemonDraftOperationSource::McpStore,
    )
    .await;
    let rule_update_org_commit = sync_local_draft_and_merge(
        &service,
        &pool,
        &update_rule,
        Some(&workflow_create_org_commit),
    )
    .await;
    let updated_rule =
        server::app::memory::get_org_memory(&pool, &common::owner_principal(&pool).await, &rule_id)
            .await
            .unwrap();
    assert_eq!(updated_rule.memory.path, "rules/memory-review-policy");
    assert_eq!(
        updated_rule.content,
        "# Memory review discipline\n\nApply when publishing durable memory.\n\nReview the change and its materialized result before merge.\n\nTags: memory, review, verification"
    );
    let rule_update_project_commit = current_project_commit_id(&pool, &bootstrap.project_id).await;
    assert_ne!(rule_update_project_commit, rule_update_org_commit);
    let rule_update_root =
        cache_root_for_commit(&service, &bootstrap.project_id, &rule_update_project_commit).await;
    assert!(
        !rule_update_root
            .join("cache/memory/rules/memory-review")
            .exists()
    );
    assert!(
        rule_update_root
            .join("cache/memory/rules/memory-review-policy")
            .exists()
    );
    assert!(
        workflow_create_root
            .join("cache/memory/rules/memory-review")
            .exists()
    );

    let update_workflow = update_and_rename_resource_draft(
        &service,
        &bootstrap.project_id,
        (DaemonDraftScope::Org, DaemonDraftResourceKind::Memory),
        &workflow_id,
        workflow_content(&format!(
            "# Memory publication\n\nPublish and verify durable memory.\n\n1. Verify the materialized generation.\n2. Apply rule `{rule_id}`."
        )),
        "workflow/memory-publish",
        DaemonDraftOperationSource::Desktop,
    )
    .await;
    let workflow_update_org_commit = sync_local_draft_and_merge(
        &service,
        &pool,
        &update_workflow,
        Some(&rule_update_org_commit),
    )
    .await;
    let updated_workflow = server::app::memory::get_org_memory(
        &pool,
        &common::owner_principal(&pool).await,
        &workflow_id,
    )
    .await
    .unwrap();
    assert_eq!(updated_workflow.memory.path, "workflow/memory-publish");
    assert_eq!(
        updated_workflow.content,
        format!(
            "# Memory publication\n\nPublish and verify durable memory.\n\n1. Verify the materialized generation.\n2. Apply rule `{rule_id}`."
        )
    );
    let workflow_update_project_commit =
        current_project_commit_id(&pool, &bootstrap.project_id).await;
    assert_ne!(workflow_update_project_commit, workflow_update_org_commit);
    let workflow_update_root = cache_root_for_commit(
        &service,
        &bootstrap.project_id,
        &workflow_update_project_commit,
    )
    .await;
    assert!(
        !workflow_update_root
            .join("cache/memory/workflow/memory-publication")
            .exists()
    );
    assert_eq!(
        std::fs::read_to_string(workflow_update_root.join("cache/memory/workflow/memory-publish"))
            .unwrap(),
        format!(
            "# Memory publication\n\nPublish and verify durable memory.\n\n1. Verify the materialized generation.\n2. Apply rule `{rule_id}`."
        )
    );
    assert!(
        workflow_create_root
            .join("cache/memory/workflow/memory-publication")
            .exists()
    );

    let delete_workflow = delete_resource_draft(
        &service,
        &bootstrap.project_id,
        DaemonDraftScope::Org,
        DaemonDraftResourceKind::Memory,
        &workflow_id,
    )
    .await;
    let workflow_delete_org_commit = sync_local_draft_and_merge(
        &service,
        &pool,
        &delete_workflow,
        Some(&workflow_update_org_commit),
    )
    .await;
    assert!(matches!(
        server::app::memory::get_org_memory(
            &pool,
            &common::owner_principal(&pool).await,
            &workflow_id
        )
        .await,
        Err(server::error::ServerError::NotFound { .. })
    ));
    let workflow_delete_project_commit =
        current_project_commit_id(&pool, &bootstrap.project_id).await;
    assert_ne!(workflow_delete_project_commit, workflow_delete_org_commit);
    let workflow_delete_root = cache_root_for_commit(
        &service,
        &bootstrap.project_id,
        &workflow_delete_project_commit,
    )
    .await;
    assert!(
        !workflow_delete_root
            .join("cache/memory/workflow/memory-publish")
            .exists()
    );
    assert!(
        workflow_update_root
            .join("cache/memory/workflow/memory-publish")
            .exists()
    );

    let delete_rule = delete_resource_draft(
        &service,
        &bootstrap.project_id,
        DaemonDraftScope::Org,
        DaemonDraftResourceKind::Memory,
        &rule_id,
    )
    .await;
    let rule_delete_org_commit = sync_local_draft_and_merge(
        &service,
        &pool,
        &delete_rule,
        Some(&workflow_delete_org_commit),
    )
    .await;
    assert!(matches!(
        server::app::memory::get_org_memory(&pool, &common::owner_principal(&pool).await, &rule_id)
            .await,
        Err(server::error::ServerError::NotFound { .. })
    ));
    let rule_delete_project_commit = current_project_commit_id(&pool, &bootstrap.project_id).await;
    assert_ne!(rule_delete_project_commit, rule_delete_org_commit);
    let rule_delete_root =
        cache_root_for_commit(&service, &bootstrap.project_id, &rule_delete_project_commit).await;
    assert!(
        !rule_delete_root
            .join("cache/memory/rules/memory-review-policy")
            .exists()
    );
    assert!(
        workflow_delete_root
            .join("cache/memory/rules/memory-review-policy")
            .exists()
    );

    server.shutdown().await;
}

#[tokio::test]
async fn selected_hub_rule_and_workflow_changes_converge_without_reselection() {
    let postgres = common::start_postgres().await;
    let database_url = &postgres.database_url;
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    server::infra::database::run_migrations(&pool)
        .await
        .unwrap();
    let bootstrap = common::initialize_installation(pool.clone(), "Selected Hub Lifecycle").await;

    let access_token = "daemon-selected-hub-lifecycle-access-token";
    let token_hash = hex::encode(Sha256::digest(access_token.as_bytes()));
    sqlx::query(
        "INSERT INTO auth_sessions (session_id, user_id, org_id)
         VALUES ('ses_daemon_selected_hub_lifecycle', $1, $2)",
    )
    .bind(&bootstrap.user_id)
    .bind(&bootstrap.org_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_tokens (
            token_id, session_id, user_id, kind, token_hash, expires_at
         ) VALUES (
            'tok_daemon_selected_hub_lifecycle', 'ses_daemon_selected_hub_lifecycle', $1,
            'access', $2, now() + interval '30 minutes'
         )",
    )
    .bind(&bootstrap.user_id)
    .bind(token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let server = common::TestServer::start(pool.clone(), postgres).await;
    let server_address = server.address;

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{server_address}");
    config.project.project_id = Some(bootstrap.project_id.clone());
    let (state, _) = common::initialize_authenticated_daemon(config, access_token, None).await;
    let service = DaemonIpcService::new(state);
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap();

    let create_rule = create_resource_draft(
        &service,
        &bootstrap.project_id,
        DaemonDraftScope::Org,
        DaemonDraftResourceKind::Memory,
        "rules/shared-review",
        rule_content(
            "# Shared review discipline\n\nReview shared memory before merge.\n\nTags: hub, review",
        ),
        DaemonDraftOperationSource::Desktop,
    )
    .await;
    let org_rule_commit = sync_local_draft_and_merge(&service, &pool, &create_rule, None).await;
    let rule = server::app::memory::list_org_memories(&pool, &common::owner_principal(&pool).await)
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|memory| memory.path == "rules/shared-review")
        .unwrap();
    let rule_id = rule.memory_id;

    let create_workflow = create_resource_draft(
        &service,
        &bootstrap.project_id,
        DaemonDraftScope::Org,
        DaemonDraftResourceKind::Memory,
        "workflow/shared-publication",
        workflow_content(&format!(
            "# Shared publication\n\nPublish organization memory safely.\n\n1. Apply rule `{rule_id}`.\n2. Confirm the effective project memory."
        )),
        DaemonDraftOperationSource::McpStore,
    )
    .await;
    let org_workflow_commit =
        sync_local_draft_and_merge(&service, &pool, &create_workflow, Some(&org_rule_commit)).await;
    let workflow =
        server::app::memory::list_org_memories(&pool, &common::owner_principal(&pool).await)
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|memory| memory.path == "workflow/shared-publication")
            .unwrap();
    let workflow_id = workflow.memory_id;

    let selection = server::app::memory::get_project_org_selection(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
    )
    .await
    .unwrap();
    assert_eq!(selection.revision, 2);
    assert_eq!(selection.memories.len(), 2);
    assert!(
        selection
            .memories
            .iter()
            .any(|memory| memory.memory_id == rule_id)
    );
    assert!(
        selection
            .memories
            .iter()
            .any(|memory| memory.memory_id == workflow_id)
    );
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap();
    let selected_project_commit = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    let selected_root =
        cache_root_for_commit(&service, &bootstrap.project_id, &selected_project_commit).await;
    assert!(
        selected_root
            .join("cache/memory/rules/shared-review")
            .exists()
    );
    assert!(
        selected_root
            .join("cache/memory/workflow/shared-publication")
            .exists()
    );

    let update_rule = update_and_rename_resource_draft(
        &service,
        &bootstrap.project_id,
        (DaemonDraftScope::Org, DaemonDraftResourceKind::Memory),
        &rule_id,
        rule_content(
            "# Shared review discipline\n\nReview shared memory and its project projections before merge.\n\nTags: hub, review, projection",
        ),
        "rules/shared-review-policy",
        DaemonDraftOperationSource::Desktop,
    )
    .await;
    let org_rule_update_commit =
        sync_local_draft_and_merge(&service, &pool, &update_rule, Some(&org_workflow_commit)).await;
    let rule_update_project_commit = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    assert_ne!(rule_update_project_commit, selected_project_commit);
    assert_ne!(rule_update_project_commit, org_rule_update_commit);
    let rule_update_root =
        cache_root_for_commit(&service, &bootstrap.project_id, &rule_update_project_commit).await;
    assert!(
        !rule_update_root
            .join("cache/memory/rules/shared-review")
            .exists()
    );
    assert_eq!(
        std::fs::read_to_string(rule_update_root.join("cache/memory/rules/shared-review-policy"))
            .unwrap(),
        "# Shared review discipline\n\nReview shared memory and its project projections before merge.\n\nTags: hub, review, projection"
    );
    assert!(
        selected_root
            .join("cache/memory/rules/shared-review")
            .exists()
    );

    let update_workflow = update_and_rename_resource_draft(
        &service,
        &bootstrap.project_id,
        (DaemonDraftScope::Org, DaemonDraftResourceKind::Memory),
        &workflow_id,
        workflow_content(&format!(
            "# Shared publication\n\nPublish and verify organization memory.\n\n1. Confirm the effective project memory.\n2. Apply rule `{rule_id}`."
        )),
        "workflow/shared-publish",
        DaemonDraftOperationSource::McpStore,
    )
    .await;
    let org_workflow_update_commit = sync_local_draft_and_merge(
        &service,
        &pool,
        &update_workflow,
        Some(&org_rule_update_commit),
    )
    .await;
    let workflow_update_project_commit = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    assert_ne!(workflow_update_project_commit, rule_update_project_commit);
    let workflow_update_root = cache_root_for_commit(
        &service,
        &bootstrap.project_id,
        &workflow_update_project_commit,
    )
    .await;
    assert!(
        !workflow_update_root
            .join("cache/memory/workflow/shared-publication")
            .exists()
    );
    assert_eq!(
        std::fs::read_to_string(workflow_update_root.join("cache/memory/workflow/shared-publish"))
            .unwrap(),
        format!(
            "# Shared publication\n\nPublish and verify organization memory.\n\n1. Confirm the effective project memory.\n2. Apply rule `{rule_id}`."
        )
    );

    let delete_workflow = delete_resource_draft(
        &service,
        &bootstrap.project_id,
        DaemonDraftScope::Org,
        DaemonDraftResourceKind::Memory,
        &workflow_id,
    )
    .await;
    let org_workflow_delete_commit = sync_local_draft_and_merge(
        &service,
        &pool,
        &delete_workflow,
        Some(&org_workflow_update_commit),
    )
    .await;
    let workflow_delete_project_commit = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    let workflow_delete_root = cache_root_for_commit(
        &service,
        &bootstrap.project_id,
        &workflow_delete_project_commit,
    )
    .await;
    assert!(
        !workflow_delete_root
            .join("cache/memory/workflow/shared-publish")
            .exists()
    );
    assert!(
        workflow_delete_root
            .join("cache/memory/rules/shared-review-policy")
            .exists()
    );
    let selection_after_workflow_delete = server::app::memory::get_project_org_selection(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
    )
    .await
    .unwrap();
    assert_eq!(
        selection_after_workflow_delete.revision,
        selection.revision + 1
    );
    assert_eq!(selection_after_workflow_delete.memories.len(), 1);
    assert_eq!(
        selection_after_workflow_delete.memories[0].memory_id,
        rule_id
    );

    let delete_rule = delete_resource_draft(
        &service,
        &bootstrap.project_id,
        DaemonDraftScope::Org,
        DaemonDraftResourceKind::Memory,
        &rule_id,
    )
    .await;
    sync_local_draft_and_merge(
        &service,
        &pool,
        &delete_rule,
        Some(&org_workflow_delete_commit),
    )
    .await;
    let rule_delete_project_commit = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    let rule_delete_root =
        cache_root_for_commit(&service, &bootstrap.project_id, &rule_delete_project_commit).await;
    assert!(
        !rule_delete_root
            .join("cache/memory/rules/shared-review-policy")
            .exists()
    );
    let final_selection = server::app::memory::get_project_org_selection(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
    )
    .await
    .unwrap();
    assert_eq!(final_selection.revision, selection.revision + 2);
    assert!(final_selection.memories.is_empty());
    assert!(
        workflow_update_root
            .join("cache/memory/workflow/shared-publish")
            .exists()
    );
    assert!(matches!(
        server::app::memory::get_org_memory(&pool, &common::owner_principal(&pool).await, &rule_id)
            .await,
        Err(server::error::ServerError::NotFound { .. })
    ));
    assert!(matches!(
        server::app::memory::get_org_memory(
            &pool,
            &common::owner_principal(&pool).await,
            &workflow_id
        )
        .await,
        Err(server::error::ServerError::NotFound { .. })
    ));

    server.shutdown().await;
}

#[tokio::test]
async fn two_daemon_installations_converge_on_the_same_draft_history() {
    let postgres = common::start_postgres().await;
    let database_url = &postgres.database_url;
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    server::infra::database::run_migrations(&pool)
        .await
        .unwrap();
    let bootstrap = common::initialize_installation(pool.clone(), "Daemon Convergence").await;

    let access_token = "daemon-convergence-access-token";
    let token_hash = hex::encode(Sha256::digest(access_token.as_bytes()));
    sqlx::query(
        "INSERT INTO auth_sessions (session_id, user_id, org_id)
         VALUES ('ses_daemon_convergence', $1, $2)",
    )
    .bind(&bootstrap.user_id)
    .bind(&bootstrap.org_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_tokens (
            token_id, session_id, user_id, kind, token_hash, expires_at
         ) VALUES (
            'tok_daemon_convergence', 'ses_daemon_convergence', $1,
            'access', $2, now() + interval '30 minutes'
         )",
    )
    .bind(&bootstrap.user_id)
    .bind(token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let server = common::TestServer::start(pool.clone(), postgres).await;
    let server_address = server.address;

    let root_a = tempfile::tempdir().unwrap();
    let mut config_a = DaemonConfig::for_root(root_a.path());
    config_a.project.server_url = format!("http://{server_address}");
    config_a.project.project_id = Some(bootstrap.project_id.clone());
    let (state_a, credential_store_a) =
        common::initialize_authenticated_daemon(config_a.clone(), access_token, None).await;
    let service_a = DaemonIpcService::new(state_a);

    let root_b = tempfile::tempdir().unwrap();
    let mut config_b = DaemonConfig::for_root(root_b.path());
    config_b.project.server_url = format!("http://{server_address}");
    config_b.project.project_id = Some(bootstrap.project_id.clone());
    let (state_b, _) = common::initialize_authenticated_daemon(config_b, access_token, None).await;
    let service_b = DaemonIpcService::new(state_b);

    let first = service_a
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/converged.md".to_owned(),
                    content: context_content("First installation"),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::McpStore),
        })
        .await
        .unwrap();
    service_a
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    service_b
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let drafts_b = service_b
        .list_drafts(DaemonDraftListQuery::default())
        .await
        .unwrap();
    assert_eq!(drafts_b.items.len(), 1);
    assert_eq!(drafts_b.items[0].server_version, 1);
    let projected_on_b = service_b
        .get_draft(&drafts_b.items[0].draft_id)
        .await
        .unwrap();
    assert_eq!(projected_on_b.operations.len(), 1);
    assert_eq!(
        projected_on_b.operations[0].source,
        DaemonDraftOperationRecordSource::Server
    );
    assert_eq!(
        projected_on_b.operations[0]
            .operation
            .create
            .as_ref()
            .unwrap()
            .content,
        context_content("First installation")
    );

    service_b
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(projected_on_b.draft.draft_id.clone()),
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/converged.md".to_owned(),
                    content: context_content("Second installation"),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();
    service_b
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    service_a
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    let converged_a = service_a.get_draft(&first.draft_id).await.unwrap();
    let converged_b = service_b
        .get_draft(&projected_on_b.draft.draft_id)
        .await
        .unwrap();
    assert_eq!(converged_a.draft.server_version, 2);
    assert_eq!(converged_b.draft.server_version, 2);
    assert_eq!(converged_a.operations.len(), 2);
    assert_eq!(converged_b.operations.len(), 2);
    assert_eq!(
        converged_a.operations[0].source,
        DaemonDraftOperationRecordSource::McpStore
    );
    assert_eq!(
        converged_a.operations[1].source,
        DaemonDraftOperationRecordSource::Server
    );
    assert_eq!(
        converged_a.operations[1]
            .operation
            .create
            .as_ref()
            .unwrap()
            .content,
        context_content("Second installation")
    );

    service_a
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    service_b
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    assert_eq!(
        service_a
            .get_draft(&first.draft_id)
            .await
            .unwrap()
            .operations
            .len(),
        2
    );

    drop(service_a);
    let restarted_a = DaemonIpcService::new(
        common::initialize_daemon(config_a, credential_store_a.clone()).await,
    );
    restarted_a
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    let after_restart = restarted_a.get_draft(&first.draft_id).await.unwrap();
    assert_eq!(after_restart.draft.server_version, 2);
    assert_eq!(after_restart.operations.len(), 2);

    server.shutdown().await;
}

#[tokio::test]
async fn merged_commit_materializes_on_two_daemons_and_survives_restart() {
    let postgres = common::start_postgres().await;
    let database_url = &postgres.database_url;
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    server::infra::database::run_migrations(&pool)
        .await
        .unwrap();
    let bootstrap = common::initialize_installation(pool.clone(), "Commit Convergence").await;

    let access_token = "daemon-commit-convergence-access-token";
    let token_hash = hex::encode(Sha256::digest(access_token.as_bytes()));
    sqlx::query(
        "INSERT INTO auth_sessions (session_id, user_id, org_id)
         VALUES ('ses_daemon_commit_convergence', $1, $2)",
    )
    .bind(&bootstrap.user_id)
    .bind(&bootstrap.org_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_tokens (
            token_id, session_id, user_id, kind, token_hash, expires_at
         ) VALUES (
            'tok_daemon_commit_convergence', 'ses_daemon_commit_convergence', $1,
            'access', $2, now() + interval '30 minutes'
         )",
    )
    .bind(&bootstrap.user_id)
    .bind(token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let secondary_project_id = server::app::project::create_project(
        &pool,
        &common::owner_principal(&pool).await,
        "Secondary Memory",
        "",
    )
    .await
    .unwrap();
    let secondary_memory_id = server::app::memory::create_org_context(
        &pool,
        &common::owner_principal(&pool).await,
        "context/secondary.md",
        "# Secondary project",
    )
    .await
    .unwrap();
    server::app::memory::replace_project_org_selection(
        &pool,
        &common::owner_principal(&pool).await,
        &secondary_project_id,
        0,
        ReplaceProjectOrgSelectionRequest {
            resource_ids: vec![secondary_memory_id],
        },
    )
    .await
    .unwrap();
    let secondary_commit_id = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &secondary_project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    let initial_org_commit_id = server::app::commit::get_org_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();

    let server = common::TestServer::start(pool.clone(), postgres).await;
    let server_address = server.address;

    let root_a = tempfile::tempdir().unwrap();
    let mut config_a = DaemonConfig::for_root(root_a.path());
    config_a.project.server_url = format!("http://{server_address}");
    config_a.project.project_id = Some(bootstrap.project_id.clone());
    let (state_a, credential_store_a) =
        common::initialize_authenticated_daemon(config_a.clone(), access_token, None).await;
    let service_a = DaemonIpcService::new(state_a);

    let root_b = tempfile::tempdir().unwrap();
    let mut config_b = DaemonConfig::for_root(root_b.path());
    config_b.project.server_url = format!("http://{server_address}");
    config_b.project.project_id = Some(bootstrap.project_id.clone());
    let (state_b, _) = common::initialize_authenticated_daemon(config_b, access_token, None).await;
    let service_b = DaemonIpcService::new(state_b);

    for service in [&service_a, &service_b] {
        service
            .retry_sync(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::Commits,
            })
            .await
            .unwrap();
        let empty = service
            .memory_cache(DaemonMemoryCacheRequest {
                project_id: bootstrap.project_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(empty.state, DaemonMemoryCacheState::Ready);
        assert_eq!(empty.commit_id, None);
        let empty_checkout = service
            .project_checkout(DaemonProjectCheckoutRequest {
                project_id: bootstrap.project_id.clone(),
            })
            .await
            .unwrap();
        assert!(empty_checkout.ready);
        assert_eq!(empty_checkout.commit_id, None);
        assert!(empty_checkout.resources.is_empty());
    }

    let draft = server::app::draft::create_draft(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        CreateDraftRequest {
            daemon_installation_id: "daemon_commit_origin".to_owned(),
            project_id: bootstrap.project_id.clone(),
            base_commit_id: Some(initial_org_commit_id.clone()),
            title: "Add synchronized context".to_owned(),
            description: None,
            resource: DraftResourceRef {
                scope: ResourceScope::Org,
                id: None,
                path: Some("context/commit-sync.md".to_owned()),
            },
            operations: vec![DraftOperationInput {
                action: DraftOperationAction::Create,
                resource: DraftResourceRef {
                    scope: ResourceScope::Org,
                    id: None,
                    path: Some("context/commit-sync.md".to_owned()),
                },
                content: Some(DraftResourceContent {
                    description: None,
                    content: "# Commit sync\n\nInstalled from an immutable Commit.".to_owned(),
                }),
                new_path: None,
            }],
        },
    )
    .await
    .unwrap();
    let review = server::app::review::create_review(
        &pool,
        &common::principal(&pool, &bootstrap.user_id).await,
        Some(&initial_org_commit_id),
        CreateReviewRequest {
            drafts: vec![ReviewDraftRequest {
                draft_id: draft.draft.draft_id,
                expected_draft_version: draft.draft.version,
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
        &pool,
        &review.review.review_id,
        &common::principal(&pool, &review.review.author.user_id).await,
        CreateReviewDecisionRequest {
            decision: ReviewDecision::Approved,
            expected_review_version: review.review.version,
            body: None,
        },
    )
    .await
    .unwrap();
    let merge = server::app::review::create_review_merge(
        &pool,
        &approved.review.review_id,
        &common::principal(&pool, &approved.review.author.user_id).await,
        Some(&initial_org_commit_id),
        CreateReviewMergeRequest {
            expected_review_version: approved.review.version,
        },
    )
    .await
    .unwrap();
    let org_commit_id = merge.commit_id.unwrap();
    let commit_sync_id =
        server::app::memory::list_org_memories(&pool, &common::owner_principal(&pool).await)
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|memory| memory.path == "context/commit-sync.md")
            .unwrap()
            .memory_id;
    let initial_selection = server::app::memory::get_project_org_selection(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
    )
    .await
    .unwrap();
    assert_eq!(initial_selection.revision, 1);
    assert_eq!(initial_selection.memories.len(), 1);
    assert_eq!(initial_selection.memories[0].memory_id, commit_sync_id);
    let org_context_id = server::app::memory::create_org_context(
        &pool,
        &common::owner_principal(&pool).await,
        "context/shared-from-hub.md",
        "# Shared from Hub\n\nSelected by the project.",
    )
    .await
    .unwrap();
    let selection = server::app::memory::replace_project_org_selection(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        initial_selection.revision,
        ReplaceProjectOrgSelectionRequest {
            resource_ids: vec![commit_sync_id.clone(), org_context_id.clone()],
        },
    )
    .await
    .unwrap();
    let commit_id = server::app::commit::get_project_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        &bootstrap.project_id,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    assert_ne!(commit_id, org_commit_id);

    let mut roots = Vec::new();
    for service in [&service_a, &service_b] {
        service
            .retry_sync(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::Commits,
            })
            .await
            .unwrap();
        let cache = service
            .memory_cache(DaemonMemoryCacheRequest {
                project_id: bootstrap.project_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(cache.state, DaemonMemoryCacheState::Ready);
        assert_eq!(cache.commit_id.as_deref(), Some(commit_id.as_str()));
        let cache_root = std::path::PathBuf::from(cache.active_generation_path.unwrap());
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(cache_root.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["project_id"], bootstrap.project_id);
        assert_eq!(manifest["commit_id"], commit_id);
        assert_eq!(
            std::fs::read_to_string(cache_root.join("cache/memory/context/commit-sync.md"))
                .unwrap(),
            "# Commit sync\n\nInstalled from an immutable Commit."
        );
        assert_eq!(
            std::fs::read_to_string(cache_root.join("cache/memory/context/shared-from-hub.md"))
                .unwrap(),
            "# Shared from Hub\n\nSelected by the project."
        );
        let checkout = service
            .project_checkout(DaemonProjectCheckoutRequest {
                project_id: bootstrap.project_id.clone(),
            })
            .await
            .unwrap();
        assert!(checkout.ready);
        assert_eq!(checkout.commit_id.as_deref(), Some(commit_id.as_str()));
        assert_eq!(checkout.org_selection_revision, selection.revision);
        assert_eq!(checkout.selected_org_resource_ids.len(), 2);
        assert!(checkout.selected_org_resource_ids.contains(&commit_sync_id));
        assert!(checkout.selected_org_resource_ids.contains(&org_context_id));
        assert!(checkout.resources.iter().any(|resource| {
            resource.scope == DaemonDraftScope::Org
                && resource.path == "context/commit-sync.md"
                && resource.content
                    == context_content("# Commit sync\n\nInstalled from an immutable Commit.")
        }));
        assert!(checkout.resources.iter().any(|resource| {
            resource.scope == DaemonDraftScope::Org && resource.path == "context/shared-from-hub.md"
        }));
        let sync = service.sync_status().await.unwrap();
        assert_eq!(sync.commit_sync.state, SyncState::Idle);
        assert_eq!(
            sync.commit_sync.server_cursor.as_deref(),
            Some(commit_id.as_str())
        );
        assert!(sync.commit_sync.last_error.is_none());
        roots.push(cache_root);
    }
    assert_ne!(roots[0], roots[1]);
    assert_eq!(
        roots[0].file_name().unwrap().to_str(),
        roots[1].file_name().unwrap().to_str()
    );

    drop(service_a);
    let restarted_a = DaemonIpcService::new(
        common::initialize_daemon(config_a, credential_store_a.clone()).await,
    );
    let cache_after_restart = restarted_a
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: bootstrap.project_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(cache_after_restart.state, DaemonMemoryCacheState::Ready);
    assert_eq!(
        cache_after_restart.commit_id.as_deref(),
        Some(commit_id.as_str())
    );
    assert_eq!(
        cache_after_restart.active_generation_path.as_deref(),
        Some(roots[0].to_str().unwrap())
    );

    let current_org_commit_id = server::app::commit::get_org_commit_state(
        &pool,
        &common::owner_principal(&pool).await,
        None,
    )
    .await
    .unwrap()
    .reference
    .commit_id
    .unwrap();
    let next_draft = restarted_a
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: bootstrap.project_id.clone(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/next.md".to_owned(),
                    content: context_content("Uses the installed Ref as its base."),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();
    let next_draft = restarted_a.get_draft(&next_draft.draft_id).await.unwrap();
    assert_eq!(
        next_draft.draft.base_commit_id.as_deref(),
        Some(current_org_commit_id.as_str())
    );

    let selected = restarted_a
        .select_project(DaemonProjectSelectionRequest {
            project_id: secondary_project_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        selected.project_id.as_deref(),
        Some(secondary_project_id.as_str())
    );
    restarted_a
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap();
    let secondary_cache = restarted_a
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: secondary_project_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        secondary_cache.commit_id.as_deref(),
        Some(secondary_commit_id.as_str())
    );
    assert_eq!(
        std::fs::read_to_string(
            std::path::PathBuf::from(secondary_cache.active_generation_path.unwrap())
                .join("cache/memory/context/secondary.md")
        )
        .unwrap(),
        "# Secondary project"
    );

    server.shutdown().await;
}
