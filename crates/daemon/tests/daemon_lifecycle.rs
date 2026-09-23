//! Isolated daemon lifecycle, persistence, and IPC contract checks.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::routing::{get, post};
use axum::{Json, Router};
use daemon::{
    ActivateMemoryRequest, CURRENT_LOCAL_SCHEMA_VERSION, DAEMON_AGENT_LABEL,
    DAEMON_MACH_SERVICE_NAME, DaemonConfig, DaemonContentDraftUpdate, DaemonCreateDraftOperation,
    DaemonDeleteDraftOperation, DaemonDiscardDraftOperation, DaemonDraftContent,
    DaemonDraftListQuery, DaemonDraftOperation, DaemonDraftOperationRecordSource,
    DaemonDraftOperationRequest, DaemonDraftOperationSource, DaemonDraftResourceKind,
    DaemonDraftScope, DaemonError, DaemonHealth, DaemonIpcRequest, DaemonIpcService,
    DaemonIpcTransport, DaemonLocalDraftStatus, DaemonMemoryCacheRequest, DaemonMemoryCacheState,
    DaemonMemoryCacheStatus, DaemonProjectAgentAdapterInstallRequest,
    DaemonProjectAgentAdapterListRequest, DaemonProjectAgentAdapterRemoveRequest,
    DaemonProjectBindingListRequest, DaemonProjectBindingRemoveRequest,
    DaemonProjectBindingReplaceRequest, DaemonProjectBindingResolveRequest,
    DaemonProjectCacheClearRequest, DaemonProjectCheckout, DaemonProjectCheckoutRequest,
    DaemonProjectConfigUpdateRequest, DaemonProjectSelectionRequest, DaemonProjectStorage,
    DaemonProjectStorageMode, DaemonProjectStorageMoveState, DaemonProjectStorageReplaceRequest,
    DaemonProjectStorageRequest, DaemonProjectStorageResetRequest, DaemonProjectSyncRetryRequest,
    DaemonProjectSyncStatusRequest, DaemonServerRequest, DaemonState, DaemonSyncRetryRequest,
    DaemonUpdateDraftOperation, DraftOperationSyncStatus, IDENTIFIER_NAMESPACE, LaunchAgentConfig,
    LaunchAgentController, LaunchAgentRuntimeStatus, ProjectAgentAdapterDelivery,
    ProjectAgentAdapterKind, ProjectAgentAdapterRuntimeRequirement, ServerCredentials,
    SyncRetryChannel, SyncState,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[cfg(target_os = "macos")]
fn directory_handoff_bookmark(path: &Path) -> String {
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
fn directory_security_bookmark(path: &Path) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use objc2_foundation::{NSString, NSURL, NSURLBookmarkCreationOptions};

    let path = NSString::from_str(&path.display().to_string());
    let url = NSURL::fileURLWithPath_isDirectory(&path, true);
    let bookmark = url
        .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions::WithSecurityScope,
            None,
            None,
        )
        .unwrap();
    STANDARD.encode(bookmark.to_vec())
}

async fn wait_for_storage_move(
    state: &DaemonState,
    move_id: &str,
) -> daemon::DaemonProjectStorageMove {
    for _ in 0..100 {
        let current = state
            .project_storage_move(daemon::DaemonProjectStorageMoveRequest {
                move_id: move_id.to_owned(),
            })
            .await
            .unwrap();
        if matches!(
            current.state,
            DaemonProjectStorageMoveState::Completed | DaemonProjectStorageMoveState::Failed
        ) {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Project storage move {move_id} did not finish");
}

fn fake_daemon_program(root: &Path) -> PathBuf {
    let path = root.join("bin/clumsiesd");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "test daemon").unwrap();
    path
}

fn signed_runtime_binary(root: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let path = root.join("Clumsies.app/Contents/Resources/clumsiesd");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy("/usr/bin/true", &path).unwrap();
        let status = std::process::Command::new("/usr/bin/codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--identifier",
                "ai.clumsies.daemon",
                "--requirements",
                "=designated => identifier \"ai.clumsies.daemon\"",
            ])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        path
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = root.join("Clumsies.app/Contents/Resources/clumsiesd");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

fn context_content(content: &str) -> DaemonDraftContent {
    DaemonDraftContent {
        org_source: None,
        description: None,
        content: content.to_owned(),
    }
}

fn rule_content(content: &str) -> DaemonDraftContent {
    DaemonDraftContent {
        org_source: None,
        description: None,
        content: content.to_owned(),
    }
}

#[tokio::test]
async fn health_initializes_local_database_and_stable_installation_id() {
    let (root, state, service) = common::test_daemon().await;
    let first_id = state.daemon_installation_id().to_owned();

    let health = service.health().await;

    assert!(health.local_db.ready);
    assert_eq!(health.local_db.schema_version, CURRENT_LOCAL_SCHEMA_VERSION);
    assert_eq!(health.daemon_installation_id, first_id);
    assert!(health.local_db.path.ends_with("local.db"));
    assert!(health.log_dir.ends_with("logs"));
    assert!(root.path().join("logs").is_dir());
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", health.local_db.path))
        .await
        .unwrap();
    let agent_tables: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('agent_runs', 'agent_run_events', 'retired_agent_runs', 'retired_agent_run_events')").fetch_one(&pool).await.unwrap();
    assert_eq!(agent_tables, 0);
    pool.close().await;

    let restarted = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        common::TestCredentialStore::default(),
    )
    .await;
    assert_eq!(restarted.daemon_installation_id(), first_id);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn project_storage_is_local_per_project_and_moves_managed_cache_safely() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let custom_a = tempfile::tempdir().unwrap();
    let custom_b = tempfile::tempdir().unwrap();
    std::fs::write(custom_a.path().join("keep.txt"), "user data").unwrap();
    let mut config = DaemonConfig::for_root(root.path().join("daemon"));
    config.project.server_url = "https://storage.example.test".to_owned();
    config.project.project_id = Some("prj_a".to_owned());
    let legacy_file = config
        .cache_dir
        .join("projects/prj_a/generations/legacy/cache/context/legacy.md");
    std::fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
    std::fs::write(&legacy_file, "legacy cache").unwrap();
    std::fs::set_permissions(
        legacy_file.parent().unwrap(),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::set_permissions(&legacy_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let state = common::initialize_daemon(config, common::TestCredentialStore::default()).await;

    let default_a = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(default_a.mode, DaemonProjectStorageMode::Default);
    assert_eq!(default_a.location_revision, 1);
    assert_eq!(
        std::fs::metadata(&default_a.managed_root_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let migrated_legacy_file =
        Path::new(&default_a.managed_root_path).join("generations/legacy/cache/context/legacy.md");
    assert!(!legacy_file.exists());
    assert_eq!(
        std::fs::metadata(&migrated_legacy_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let move_a = state
        .replace_project_storage(DaemonProjectStorageReplaceRequest {
            project_id: "prj_a".to_owned(),
            selected_root_path: custom_a.path().display().to_string(),
            handoff_bookmark_data: directory_handoff_bookmark(custom_a.path()),
            expected_location_revision: default_a.location_revision,
        })
        .await
        .unwrap();
    let move_a = wait_for_storage_move(&state, &move_a.move_id).await;
    assert_eq!(
        move_a.state,
        DaemonProjectStorageMoveState::Completed,
        "storage move failed: {:?} {:?}",
        move_a.error_code,
        move_a.error_message
    );

    let custom_status_a = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(custom_status_a.mode, DaemonProjectStorageMode::Custom);
    assert_eq!(custom_status_a.location_revision, 2);
    assert!(
        Path::new(&custom_status_a.managed_root_path)
            .starts_with(std::fs::canonicalize(custom_a.path()).unwrap())
    );
    assert!(Path::new(&custom_status_a.search_index_path).is_file());

    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE project_storage_locations SET bookmark_data = 'aW52YWxpZA=='
         WHERE project_id = 'prj_a'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let invalid_authorization = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        invalid_authorization.availability,
        daemon::DaemonProjectStorageAvailability::Unavailable
    );
    assert_eq!(
        invalid_authorization.issue_code.as_deref(),
        Some("project_storage_corrupt")
    );

    let refreshed_authorization = state
        .replace_project_storage(DaemonProjectStorageReplaceRequest {
            project_id: "prj_a".to_owned(),
            selected_root_path: custom_a.path().display().to_string(),
            handoff_bookmark_data: directory_handoff_bookmark(custom_a.path()),
            expected_location_revision: custom_status_a.location_revision,
        })
        .await
        .unwrap();
    assert_eq!(
        refreshed_authorization.state,
        DaemonProjectStorageMoveState::Completed
    );
    let refreshed_status_a = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        refreshed_status_a.availability,
        daemon::DaemonProjectStorageAvailability::Ready
    );
    assert_eq!(refreshed_status_a.location_revision, 3);

    std::fs::set_permissions(custom_a.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let unavailable = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        unavailable.availability,
        daemon::DaemonProjectStorageAvailability::Unavailable
    );
    assert_eq!(
        unavailable.issue_code.as_deref(),
        Some("permission_required")
    );
    let service = DaemonIpcService::new(state.clone());
    let offline_draft = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_a".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/offline.md".to_owned(),
                    content: context_content("Draft remains available while storage is offline"),
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
    assert!(offline_draft.queued);
    std::fs::set_permissions(custom_a.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        state
            .project_storage(DaemonProjectStorageRequest {
                project_id: "prj_a".to_owned(),
            })
            .await
            .unwrap()
            .availability,
        daemon::DaemonProjectStorageAvailability::Ready
    );

    let default_b = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_b".to_owned(),
        })
        .await
        .unwrap();
    let move_b = state
        .replace_project_storage(DaemonProjectStorageReplaceRequest {
            project_id: "prj_b".to_owned(),
            selected_root_path: custom_a.path().display().to_string(),
            handoff_bookmark_data: directory_handoff_bookmark(custom_a.path()),
            expected_location_revision: default_b.location_revision,
        })
        .await
        .unwrap();
    let move_b = wait_for_storage_move(&state, &move_b.move_id).await;
    assert_eq!(
        move_b.state,
        DaemonProjectStorageMoveState::Completed,
        "storage move failed: {:?} {:?}",
        move_b.error_code,
        move_b.error_message
    );
    let still_custom_a = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        still_custom_a.managed_root_path,
        custom_status_a.managed_root_path
    );
    let custom_status_b = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_b".to_owned(),
        })
        .await
        .unwrap();
    assert_ne!(
        custom_status_b.managed_root_path,
        custom_status_a.managed_root_path
    );

    let stale = state
        .replace_project_storage(DaemonProjectStorageReplaceRequest {
            project_id: "prj_a".to_owned(),
            selected_root_path: custom_b.path().display().to_string(),
            handoff_bookmark_data: directory_handoff_bookmark(custom_b.path()),
            expected_location_revision: 1,
        })
        .await
        .unwrap_err();
    assert!(stale.to_string().contains("storage_move_conflict"));

    state
        .clear_project_cache(DaemonProjectCacheClearRequest {
            project_id: "prj_a".to_owned(),
            expected_location_revision: refreshed_status_a.location_revision,
        })
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(custom_a.path().join("keep.txt")).unwrap(),
        "user data"
    );

    let reset = state
        .reset_project_storage(DaemonProjectStorageResetRequest {
            project_id: "prj_a".to_owned(),
            expected_location_revision: refreshed_status_a.location_revision,
        })
        .await
        .unwrap();
    assert_eq!(
        wait_for_storage_move(&state, &reset.move_id).await.state,
        DaemonProjectStorageMoveState::Completed
    );
    let reset_status = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(reset_status.mode, DaemonProjectStorageMode::Default);
    assert_eq!(reset_status.location_revision, 4);
    assert!(!Path::new(&custom_status_a.managed_root_path).exists());
    assert!(Path::new(&custom_status_b.managed_root_path).exists());

    let central_search_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name IN ('search_resources', 'search_units', 'search_revisions')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(central_search_tables, 0);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn stale_bookmark_is_repaired_from_the_selected_path() {
    let root = tempfile::tempdir().unwrap();
    let custom = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path().join("daemon"));
    config.project.server_url = "https://storage.example.test".to_owned();
    config.project.project_id = Some("prj_stale".to_owned());
    let state = common::initialize_daemon(config, common::TestCredentialStore::default()).await;

    let default = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_stale".to_owned(),
        })
        .await
        .unwrap();
    let moved = state
        .replace_project_storage(DaemonProjectStorageReplaceRequest {
            project_id: "prj_stale".to_owned(),
            selected_root_path: custom.path().display().to_string(),
            handoff_bookmark_data: directory_handoff_bookmark(custom.path()),
            expected_location_revision: default.location_revision,
        })
        .await
        .unwrap();
    assert_eq!(
        wait_for_storage_move(&state, &moved.move_id).await.state,
        DaemonProjectStorageMoveState::Completed
    );

    // Simulate the workspace migration: the stored bookmark references a file
    // identity that no longer exists while the selected path stays reachable.
    let dead_dir = tempfile::tempdir().unwrap();
    let dead_bookmark = directory_security_bookmark(dead_dir.path());
    drop(dead_dir);
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE project_storage_locations SET bookmark_data = $1
         WHERE project_id = 'prj_stale'",
    )
    .bind(&dead_bookmark)
    .execute(&pool)
    .await
    .unwrap();

    let repaired = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_stale".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        repaired.availability,
        daemon::DaemonProjectStorageAvailability::Ready
    );
    let refreshed_bookmark: Option<String> = sqlx::query_scalar(
        "SELECT bookmark_data FROM project_storage_locations WHERE project_id = 'prj_stale'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(refreshed_bookmark.as_deref(), Some(dead_bookmark.as_str()));
    let persisted_root: String = sqlx::query_scalar(
        "SELECT selected_root_path FROM project_storage_locations WHERE project_id = 'prj_stale'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        Path::new(&persisted_root),
        std::fs::canonicalize(custom.path()).unwrap()
    );

    // The repaired authorization keeps working on later resolutions.
    let again = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_stale".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        again.availability,
        daemon::DaemonProjectStorageAvailability::Ready
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn stale_bookmark_with_unreachable_selected_path_reports_volume_missing() {
    let root = tempfile::tempdir().unwrap();
    let custom = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path().join("daemon"));
    config.project.server_url = "https://storage.example.test".to_owned();
    config.project.project_id = Some("prj_stale_gone".to_owned());
    let state = common::initialize_daemon(config, common::TestCredentialStore::default()).await;

    let default = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_stale_gone".to_owned(),
        })
        .await
        .unwrap();
    let moved = state
        .replace_project_storage(DaemonProjectStorageReplaceRequest {
            project_id: "prj_stale_gone".to_owned(),
            selected_root_path: custom.path().display().to_string(),
            handoff_bookmark_data: directory_handoff_bookmark(custom.path()),
            expected_location_revision: default.location_revision,
        })
        .await
        .unwrap();
    assert_eq!(
        wait_for_storage_move(&state, &moved.move_id).await.state,
        DaemonProjectStorageMoveState::Completed
    );

    let dead_dir = tempfile::tempdir().unwrap();
    let dead_bookmark = directory_security_bookmark(dead_dir.path());
    drop(dead_dir);
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE project_storage_locations SET bookmark_data = $1
         WHERE project_id = 'prj_stale_gone'",
    )
    .bind(&dead_bookmark)
    .execute(&pool)
    .await
    .unwrap();
    std::fs::remove_dir_all(custom.path()).unwrap();

    let unavailable = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_stale_gone".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        unavailable.availability,
        daemon::DaemonProjectStorageAvailability::Unavailable
    );
    assert_eq!(unavailable.issue_code.as_deref(), Some("volume_missing"));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn project_storage_moves_resume_from_every_nonterminal_state() {
    for interrupted_state in [
        "preparing",
        "materializing",
        "verifying",
        "switching",
        "cleaning",
    ] {
        let root = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        let mut config = DaemonConfig::for_root(root.path().join("daemon"));
        config.project.server_url = "https://storage-recovery.example.test".to_owned();
        config.project.project_id = Some("prj_recovery".to_owned());
        let state =
            common::initialize_daemon(config.clone(), common::TestCredentialStore::default()).await;
        let source = state
            .project_storage(DaemonProjectStorageRequest {
                project_id: "prj_recovery".to_owned(),
            })
            .await
            .unwrap();

        let (move_id, expected_destination) = if interrupted_state == "cleaning" {
            let created = state
                .replace_project_storage(DaemonProjectStorageReplaceRequest {
                    project_id: "prj_recovery".to_owned(),
                    selected_root_path: custom.path().display().to_string(),
                    handoff_bookmark_data: directory_handoff_bookmark(custom.path()),
                    expected_location_revision: source.location_revision,
                })
                .await
                .unwrap();
            let completed = wait_for_storage_move(&state, &created.move_id).await;
            assert_eq!(completed.state, DaemonProjectStorageMoveState::Completed);
            let destination = state
                .project_storage(DaemonProjectStorageRequest {
                    project_id: "prj_recovery".to_owned(),
                })
                .await
                .unwrap();
            let source_root = Path::new(&completed.source_managed_root_path);
            std::fs::create_dir_all(source_root.join("generations")).unwrap();
            std::fs::create_dir_all(source_root.join("search")).unwrap();
            std::fs::create_dir_all(source_root.join("staging")).unwrap();
            std::fs::copy(
                Path::new(&destination.managed_root_path).join("ownership.json"),
                source_root.join("ownership.json"),
            )
            .unwrap();
            std::fs::write(source_root.join("obsolete-cache"), "remove after restart").unwrap();
            let pool =
                sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
                    .await
                    .unwrap();
            sqlx::query(
                "UPDATE project_storage_moves
                 SET state = 'cleaning', completed_at = NULL
                 WHERE move_id = $1",
            )
            .bind(&created.move_id)
            .execute(&pool)
            .await
            .unwrap();
            // Simulate a crash immediately after `switch_location`: the
            // central mirror still names the source location and no durable
            // build request was committed yet. Cleaning recovery must repair
            // both before it removes the source cache.
            sqlx::query(
                "INSERT INTO search_heads (
                    project_id, revision_id, effective_hash, status,
                    location_revision
                 ) VALUES ('prj_recovery', 'stale_source_head',
                           'stale_effective_hash', 'ready', $1)
                 ON CONFLICT(project_id) DO UPDATE SET
                    revision_id = excluded.revision_id,
                    effective_hash = excluded.effective_hash,
                    status = excluded.status,
                    location_revision = excluded.location_revision",
            )
            .bind(source.location_revision)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("DELETE FROM search_index_jobs WHERE project_id = 'prj_recovery'")
                .execute(&pool)
                .await
                .unwrap();
            pool.close().await;
            (created.move_id, destination.managed_root_path)
        } else {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let selected_root = std::fs::canonicalize(custom.path()).unwrap();
            let authority_hash = hex::encode(Sha256::digest(source.authority_key.as_bytes()));
            let destination_root = selected_root
                .join(".clumsies/cache-v1")
                .join(authority_hash)
                .join("prj_recovery");
            let move_id = format!("move_recovery_{interrupted_state}");
            let pool =
                sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
                    .await
                    .unwrap();
            sqlx::query(
                "INSERT INTO project_storage_moves (
                    move_id, authority_key, project_id,
                    source_mode, source_selected_root_path, source_bookmark_data,
                    source_managed_root_path, source_location_revision,
                    destination_mode, destination_selected_root_path,
                    destination_bookmark_data, destination_managed_root_path, state
                 ) VALUES ($1, $2, 'prj_recovery', 'default', $3, NULL, $4, $5,
                           'custom', $6, $7, $8, $9)",
            )
            .bind(&move_id)
            .bind(&source.authority_key)
            .bind(&source.selected_root_path)
            .bind(&source.managed_root_path)
            .bind(source.location_revision)
            .bind(selected_root.display().to_string())
            .bind(directory_security_bookmark(custom.path()))
            .bind(destination_root.display().to_string())
            .bind(interrupted_state)
            .execute(&pool)
            .await
            .unwrap();
            pool.close().await;
            (move_id, destination_root.display().to_string())
        };

        let source_root = source.managed_root_path.clone();
        drop(state);
        let restarted =
            common::initialize_daemon(config, common::TestCredentialStore::default()).await;
        let recovered = wait_for_storage_move(&restarted, &move_id).await;
        assert_eq!(
            recovered.state,
            DaemonProjectStorageMoveState::Completed,
            "move interrupted in {interrupted_state} failed: {:?} {:?}",
            recovered.error_code,
            recovered.error_message
        );
        let storage = restarted
            .project_storage(DaemonProjectStorageRequest {
                project_id: "prj_recovery".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(storage.mode, DaemonProjectStorageMode::Custom);
        assert_eq!(storage.managed_root_path, expected_destination);
        assert!(!Path::new(&source_root).exists());
        if interrupted_state == "cleaning" {
            let pool = sqlx::SqlitePool::connect(&format!(
                "sqlite://{}",
                restarted.local_db_path().display()
            ))
            .await
            .unwrap();
            let stale_head_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM search_heads
                 WHERE project_id = 'prj_recovery'
                   AND location_revision <> $1",
            )
            .bind(storage.location_revision)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(stale_head_count, 0);
            let desired_sequence: i64 = sqlx::query_scalar(
                "SELECT desired_sequence FROM search_index_jobs
                 WHERE project_id = 'prj_recovery'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(desired_sequence >= 1);
            pool.close().await;
        }
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn cleanup_failure_keeps_the_switched_location_active_and_reports_a_warning() {
    let root = tempfile::tempdir().unwrap();
    let custom = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path().join("daemon"));
    config.project.server_url = "https://storage-cleanup.example.test".to_owned();
    config.project.project_id = Some("prj_cleanup".to_owned());
    let state =
        common::initialize_daemon(config.clone(), common::TestCredentialStore::default()).await;
    let source = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_cleanup".to_owned(),
        })
        .await
        .unwrap();
    let created = state
        .replace_project_storage(DaemonProjectStorageReplaceRequest {
            project_id: "prj_cleanup".to_owned(),
            selected_root_path: custom.path().display().to_string(),
            handoff_bookmark_data: directory_handoff_bookmark(custom.path()),
            expected_location_revision: source.location_revision,
        })
        .await
        .unwrap();
    assert_eq!(
        wait_for_storage_move(&state, &created.move_id).await.state,
        DaemonProjectStorageMoveState::Completed
    );
    let destination = state
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_cleanup".to_owned(),
        })
        .await
        .unwrap();

    let source_root = Path::new(&source.managed_root_path);
    std::fs::create_dir_all(source_root).unwrap();
    std::fs::write(
        source_root.join("ownership.json"),
        serde_json::to_vec(&json!({
            "layout_version": 1,
            "authority_key": source.authority_key,
            "project_id": "another_project"
        }))
        .unwrap(),
    )
    .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE project_storage_moves
         SET state = 'cleaning', completed_at = NULL
         WHERE move_id = $1",
    )
    .bind(&created.move_id)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    drop(state);

    let restarted = common::initialize_daemon(config, common::TestCredentialStore::default()).await;
    let recovered = wait_for_storage_move(&restarted, &created.move_id).await;
    assert_eq!(recovered.state, DaemonProjectStorageMoveState::Completed);
    assert_eq!(
        recovered.error_code.as_deref(),
        Some("storage_cleanup_failed")
    );
    let status = restarted
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_cleanup".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(status.managed_root_path, destination.managed_root_path);
    assert_eq!(
        status.availability,
        daemon::DaemonProjectStorageAvailability::Ready
    );
    assert_eq!(status.issue_code.as_deref(), Some("storage_cleanup_failed"));
    assert!(source_root.exists());
}

#[tokio::test]
async fn default_project_storage_is_authority_scoped_and_refuses_marker_reassignment() {
    let root = tempfile::tempdir().unwrap();
    let mut config_a = DaemonConfig::for_root(root.path().join("daemon"));
    config_a.project.server_url = "https://authority-a.example.test".to_owned();
    config_a.project.project_id = Some("prj_same".to_owned());
    let state_a = common::initialize_daemon(config_a, common::TestCredentialStore::default()).await;
    let storage_a = state_a
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_same".to_owned(),
        })
        .await
        .unwrap();
    drop(state_a);

    let mut config_b = DaemonConfig::for_root(root.path().join("daemon"));
    config_b.project.server_url = "https://authority-b.example.test".to_owned();
    config_b.project.project_id = Some("prj_same".to_owned());
    let state_b = common::initialize_daemon(config_b, common::TestCredentialStore::default()).await;
    let storage_b = state_b
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_same".to_owned(),
        })
        .await
        .unwrap();
    assert_ne!(storage_a.managed_root_path, storage_b.managed_root_path);
    assert!(Path::new(&storage_a.managed_root_path).exists());
    assert!(Path::new(&storage_b.managed_root_path).exists());

    std::fs::write(
        Path::new(&storage_b.managed_root_path).join("ownership.json"),
        serde_json::to_vec(&json!({
            "layout_version": 1,
            "authority_key": storage_a.authority_key,
            "project_id": "prj_same"
        }))
        .unwrap(),
    )
    .unwrap();
    let unavailable = state_b
        .project_storage(DaemonProjectStorageRequest {
            project_id: "prj_same".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        unavailable.availability,
        daemon::DaemonProjectStorageAvailability::Unavailable
    );
    assert_eq!(unavailable.issue_code.as_deref(), Some("marker_mismatch"));
    assert_eq!(unavailable.managed_root_path, storage_b.managed_root_path);
}

#[tokio::test]
async fn initialization_rejects_an_old_local_schema() {
    let root = tempfile::tempdir().unwrap();
    let database_path = root.path().join("local.db");
    std::fs::File::create(&database_path).unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", database_path.display()))
        .await
        .unwrap();
    sqlx::query("CREATE TABLE daemon_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO daemon_meta (key, value) VALUES ('schema_version', '3')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let error = DaemonState::initialize_with_credential_store(
        DaemonConfig::for_root(root.path()),
        Arc::new(common::TestCredentialStore::default()),
    )
    .await
    .err()
    .unwrap();

    assert!(matches!(error, DaemonError::InvalidConfig(_)));
    assert!(error.to_string().contains("recreate the daemon database"));
}

#[tokio::test]
async fn schema_13_migration_removes_metaprompt_and_resets_rebuildable_memory_state() {
    let root = tempfile::tempdir().unwrap();
    let database_path = root.path().join("local.db");
    std::fs::File::create(&database_path).unwrap();
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    for statement in [
        "CREATE TABLE daemon_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "INSERT INTO daemon_meta (key, value) VALUES
            ('schema_version', '13'),
            ('search_schema_version', '1'),
            ('draft_events_cursor', 'cursor_old')",
        "CREATE TABLE local_drafts (
            draft_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            server_draft_id TEXT,
            server_version BIGINT NOT NULL DEFAULT 0,
            base_commit_id TEXT,
            resource_scope TEXT NOT NULL CHECK (resource_scope IN ('org', 'project')),
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'metaprompt')),
            target_id TEXT,
            path TEXT,
            conflict_base_commit_id TEXT,
            conflict_current_commit_id TEXT,
            conflicted_at TEXT,
            status TEXT NOT NULL CHECK (status IN ('open', 'submitted', 'discarded', 'conflicted', 'merged')) DEFAULT 'open',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE local_draft_operations (
            local_operation_id TEXT PRIMARY KEY,
            draft_id TEXT NOT NULL REFERENCES local_drafts(draft_id) ON DELETE CASCADE,
            server_operation_id TEXT,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'metaprompt')),
            operation_json TEXT NOT NULL,
            source TEXT NOT NULL CHECK (source IN ('desktop', 'cli', 'mcp_store', 'server')),
            sync_status TEXT NOT NULL CHECK (sync_status IN ('queued', 'syncing', 'retrying', 'synced', 'failed')),
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "INSERT INTO local_drafts (
            draft_id, project_id, resource_scope, resource_kind, path, status,
            created_at, updated_at
         ) VALUES
            ('draft_context', 'project', 'project', 'context', 'context/keep.md', 'open', 'now', 'now'),
            ('draft_rule', 'project', 'project', 'rule', 'rules/testing.md', 'open', 'now', 'now'),
            ('draft_metaprompt', 'project', 'project', 'metaprompt', 'META_PROMPT.md', 'open', 'now', 'now')",
        r##"INSERT INTO local_draft_operations (
            local_operation_id, draft_id, resource_kind, operation_json, source,
            sync_status, created_at, updated_at
         ) VALUES
            ('operation_context', 'draft_context', 'context', '{}', 'desktop', 'queued', 'now', 'now'),
            ('operation_rule', 'draft_rule', 'rule', '{"create":{"path":"rules/testing.md","content":{"kind":"rule","name":"Testing","applies_when":"While coding","constraint":"# Testing\n\nRun focused tests.","tags":["testing"]},"description":null},"update":null,"rename":null,"delete":null,"discard":null}', 'desktop', 'queued', 'now', 'now'),
            ('operation_metaprompt', 'draft_metaprompt', 'metaprompt', '{}', 'desktop', 'queued', 'now', 'now')"##,
        "CREATE TABLE cached_blobs (
            blob_id TEXT PRIMARY KEY,
            content TEXT NOT NULL
        )",
        "CREATE TABLE cached_trees (
            tree_id TEXT PRIMARY KEY,
            payload_json TEXT NOT NULL
        )",
        "CREATE TABLE cached_commits (
            commit_id TEXT PRIMARY KEY,
            scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
            org_id TEXT NOT NULL,
            project_id TEXT,
            tree_id TEXT NOT NULL,
            parent_commit_id TEXT,
            version BIGINT NOT NULL,
            created_at TEXT NOT NULL,
            payload_json TEXT NOT NULL
        )",
        "CREATE TABLE cached_refs (
            ref_key TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
            org_id TEXT NOT NULL,
            project_id TEXT,
            commit_id TEXT,
            etag TEXT NOT NULL,
            server_updated_at TEXT NOT NULL,
            installed_at TEXT NOT NULL
        )",
        "INSERT INTO cached_blobs (blob_id, content)
         VALUES ('blob_old', 'obsolete bootstrap')",
        "INSERT INTO cached_trees (tree_id, payload_json)
         VALUES ('tree_old', '{\"tree_id\":\"tree_old\",\"entries\":[{\"type\":\"metaprompt\"}]}')",
        "INSERT INTO cached_commits (
            commit_id, scope, org_id, tree_id, version, created_at, payload_json
         ) VALUES ('commit_old', 'org', 'org', 'tree_old', 1, 'now', '{}')",
        "INSERT INTO cached_refs (
            ref_key, name, scope, org_id, commit_id, etag, server_updated_at, installed_at
         ) VALUES ('org:org', 'refs/heads/main', 'org', 'org', 'commit_old', 'commit_old', 'now', 'now')",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let stale_generation = root
        .path()
        .join("cache/projects/project/generations/commit_old");
    std::fs::create_dir_all(&stale_generation).unwrap();
    std::fs::write(
        stale_generation.join("META_PROMPT.md"),
        "obsolete bootstrap",
    )
    .unwrap();

    let _state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        common::TestCredentialStore::default(),
    )
    .await;
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();

    let schema_version: String =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'schema_version'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(schema_version, CURRENT_LOCAL_SCHEMA_VERSION.to_string());
    let drafts: Vec<String> =
        sqlx::query_scalar("SELECT draft_id FROM local_drafts ORDER BY draft_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(drafts, vec!["draft_context", "draft_rule"]);
    let operations: Vec<String> = sqlx::query_scalar(
        "SELECT local_operation_id FROM local_draft_operations ORDER BY local_operation_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(operations, vec!["operation_context", "operation_rule"]);
    let migrated_rule_operation: String = sqlx::query_scalar(
        "SELECT operation_json
         FROM local_draft_operations
         WHERE local_operation_id = 'operation_rule'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let migrated_rule_operation: serde_json::Value =
        serde_json::from_str(&migrated_rule_operation).unwrap();
    assert_eq!(
        migrated_rule_operation["create"]["content"],
        json!({
            "kind": "rule",
            "content": "# Testing\n\n## Applies when\n\nWhile coding\n\n## Constraint\n\n# Testing\n\nRun focused tests.\n\nTags: testing"
        })
    );
    for table in [
        "cached_refs",
        "cached_commits",
        "cached_trees",
        "cached_blobs",
    ] {
        let query = format!("SELECT COUNT(*) FROM {table}");
        let count: i64 = sqlx::query_scalar(&query).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 0, "{table} was not reset");
    }
    let replay_cursor: Option<String> =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'draft_events_cursor'")
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(replay_cursor.is_none());
    let reset_marker: Option<String> = sqlx::query_scalar(
        "SELECT value FROM daemon_meta WHERE key = 'memory_cache_reset_required'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(reset_marker.is_none());
    let search_schema_version: String =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'search_schema_version'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(search_schema_version, "3");
    assert!(!root.path().join("cache/projects").exists());

    let rejected = sqlx::query(
        "INSERT INTO local_drafts (
            draft_id, project_id, resource_scope, resource_kind, status
         ) VALUES ('draft_rejected', 'project', 'project', 'metaprompt', 'open')",
    )
    .execute(&pool)
    .await;
    assert!(rejected.is_err());
}

#[tokio::test]
async fn schema_14_migration_separates_conflicted_lifecycle_from_coordination() {
    let root = tempfile::tempdir().unwrap();
    let database_path = root.path().join("local.db");
    std::fs::File::create(&database_path).unwrap();
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    for statement in [
        "CREATE TABLE daemon_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "INSERT INTO daemon_meta (key, value) VALUES ('schema_version', '14')",
        "CREATE TABLE local_drafts (
            draft_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            server_draft_id TEXT,
            server_version BIGINT NOT NULL DEFAULT 0,
            base_commit_id TEXT,
            resource_scope TEXT NOT NULL CHECK (resource_scope IN ('org', 'project')),
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow')),
            target_id TEXT,
            path TEXT,
            conflict_base_commit_id TEXT,
            conflict_current_commit_id TEXT,
            conflicted_at TEXT,
            status TEXT NOT NULL CHECK (status IN ('open', 'submitted', 'discarded', 'conflicted', 'merged')) DEFAULT 'open',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE local_draft_operations (
            local_operation_id TEXT PRIMARY KEY,
            draft_id TEXT NOT NULL REFERENCES local_drafts(draft_id) ON DELETE CASCADE,
            server_operation_id TEXT,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow')),
            operation_json TEXT NOT NULL,
            source TEXT NOT NULL CHECK (source IN ('desktop', 'cli', 'mcp_store', 'server')),
            sync_status TEXT NOT NULL CHECK (sync_status IN ('queued', 'syncing', 'retrying', 'synced', 'failed')),
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "INSERT INTO local_drafts (
            draft_id, project_id, server_draft_id, server_version, base_commit_id,
            resource_scope, resource_kind, target_id, path,
            conflict_base_commit_id, conflict_current_commit_id, conflicted_at,
            status, created_at, updated_at
         ) VALUES (
            'draft_conflicted', 'project_test', 'server_draft', 4, 'commit_base',
            'project', 'context', 'context_test', 'context/test.md',
            'commit_base', 'commit_current', '2026-07-22T00:00:00Z',
            'conflicted', '2026-07-22T00:00:00Z', '2026-07-22T00:01:00Z'
         )",
        r#"INSERT INTO local_draft_operations (
            local_operation_id, draft_id, server_operation_id, resource_kind,
            operation_json, source, sync_status, created_at, updated_at
         ) VALUES (
            'operation_test', 'draft_conflicted', 'server_operation', 'context',
            '{"create":null,"update":{"id":"context_test","content":{"kind":"context","content":"Draft body"},"description":null},"rename":null,"delete":null,"discard":null}',
            'desktop', 'synced', '2026-07-22T00:00:00Z', '2026-07-22T00:01:00Z'
         )"#,
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let _state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        common::TestCredentialStore::default(),
    )
    .await;
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    let row: (
        Option<String>,
        Option<String>,
        String,
        String,
        Option<String>,
        String,
    ) = sqlx::query_as(
        "SELECT base_commit_id, current_commit_id, freshness, reconciliation,
                    reconciliation_candidate_id, status
             FROM local_drafts WHERE draft_id = 'draft_conflicted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0.as_deref(), Some("commit_base"));
    assert_eq!(row.1.as_deref(), Some("commit_current"));
    assert_eq!(row.2, "behind");
    assert_eq!(row.3, "unknown");
    assert_eq!(row.4, None);
    assert_eq!(row.5, "submitted");
    let operation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM local_draft_operations WHERE draft_id = 'draft_conflicted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(operation_count, 1);
    assert!(
        sqlx::query(
            "UPDATE local_drafts SET status = 'conflicted' WHERE draft_id = 'draft_conflicted'"
        )
        .execute(&pool)
        .await
        .is_err()
    );
    let project_bindings_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name = 'project_bindings'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(project_bindings_table, 1);
}

#[tokio::test]
async fn schema_16_migration_creates_project_storage_registry_and_removes_central_search_data() {
    let root = tempfile::tempdir().unwrap();
    let database_path = root.path().join("local.db");
    std::fs::File::create(&database_path).unwrap();
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    for statement in [
        "CREATE TABLE daemon_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "INSERT INTO daemon_meta (key, value) VALUES
            ('schema_version', '16'),
            ('search_schema_version', '2')",
        "CREATE TABLE search_revisions (revision_id TEXT PRIMARY KEY)",
        "CREATE TABLE search_resources (resource_id TEXT PRIMARY KEY)",
        "CREATE TABLE search_units (unit_rowid INTEGER PRIMARY KEY)",
        "CREATE TABLE search_heads (project_id TEXT PRIMARY KEY)",
        "INSERT INTO search_revisions (revision_id) VALUES ('legacy_revision')",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let _state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        common::TestCredentialStore::default(),
    )
    .await;
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();

    let schema_version: String =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'schema_version'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(schema_version, CURRENT_LOCAL_SCHEMA_VERSION.to_string());
    for table in ["project_storage_locations", "project_storage_moves"] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = $1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "{table} was not created");
    }
    for table in ["search_revisions", "search_resources", "search_units"] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = $1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0, "legacy {table} was not removed");
    }
    let central_head_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('search_heads') ORDER BY cid")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(central_head_columns.contains(&"location_revision".to_owned()));
}

#[tokio::test]
async fn schema_17_migration_adds_retrieval_history_and_restart_recovers_running_runs() {
    let root = tempfile::tempdir().unwrap();
    let database_path = root.path().join("local.db");
    std::fs::File::create(&database_path).unwrap();
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    for statement in [
        "CREATE TABLE daemon_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "INSERT INTO daemon_meta (key, value) VALUES ('schema_version', '17')",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        common::TestCredentialStore::default(),
    )
    .await;
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    let schema_version: String =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'schema_version'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(schema_version, CURRENT_LOCAL_SCHEMA_VERSION.to_string());
    for table in [
        "retrieval_runs",
        "retrieval_run_candidates",
        "retrieval_corpus_blobs",
        "retrieval_run_resources",
        "evaluation_corpora",
        "evaluation_corpus_resources",
        "evaluation_cases",
        "evaluation_evidence",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = $1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "{table} was not created");
    }
    sqlx::query(
        "INSERT INTO retrieval_runs (
            run_id, project_id, query, activation_state_fingerprint, status
         ) VALUES (
            'run_interrupted', 'project_test', 'unfinished query',
            'sha256:test', 'running'
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    drop(state);

    let _restarted = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        common::TestCredentialStore::default(),
    )
    .await;
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    let recovered: (String, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status, error_stage, error_code, completed_at
         FROM retrieval_runs WHERE run_id = 'run_interrupted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recovered.0, "failed");
    assert_eq!(recovered.1.as_deref(), Some("interrupted"));
    assert_eq!(recovered.2.as_deref(), Some("retrieval_interrupted"));
    assert!(recovered.3.is_some());
}

#[tokio::test]
async fn schema_20_migration_preserves_drafts_and_retires_agent_runs() {
    let root = tempfile::tempdir().unwrap();
    let database_path = root.path().join("local.db");
    std::fs::File::create(&database_path).unwrap();
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    for statement in [
        "CREATE TABLE daemon_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "INSERT INTO daemon_meta (key, value) VALUES ('schema_version', '20')",
        "CREATE TABLE local_drafts (
            draft_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            server_draft_id TEXT,
            server_version BIGINT NOT NULL DEFAULT 0,
            base_commit_id TEXT,
            current_commit_id TEXT,
            freshness TEXT NOT NULL CHECK (freshness IN ('current', 'behind')) DEFAULT 'current',
            reconciliation TEXT NOT NULL CHECK (reconciliation IN ('unknown', 'clean', 'conflicts')) DEFAULT 'unknown',
            reconciliation_candidate_id TEXT,
            resource_scope TEXT NOT NULL CHECK (resource_scope IN ('org', 'project')),
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow')),
            target_id TEXT,
            path TEXT,
            status TEXT NOT NULL CHECK (status IN ('open', 'submitted', 'merged', 'discarded')) DEFAULT 'open',
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let _state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        common::TestCredentialStore::default(),
    )
    .await;
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    let schema_version: String =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'schema_version'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(schema_version, CURRENT_LOCAL_SCHEMA_VERSION.to_string());
    let change_flag_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('local_drafts')
         WHERE name = 'has_upstream_resource_changes'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(change_flag_count, 1);
    let agent_runs_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent_runs'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(agent_runs_count, 0);
}

#[tokio::test]
async fn schema_21_migration_retires_agent_run_tracking_without_an_issue_table() {
    let root = tempfile::tempdir().unwrap();
    let database_path = root.path().join("local.db");
    std::fs::File::create(&database_path).unwrap();
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    sqlx::query("CREATE TABLE daemon_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO daemon_meta (key, value) VALUES ('schema_version', '21')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let _state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        common::TestCredentialStore::default(),
    )
    .await;
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    let schema_version: String =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'schema_version'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(schema_version, CURRENT_LOCAL_SCHEMA_VERSION.to_string());
    for table in ["agent_runs", "agent_run_events"] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = $1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0, "{table} was not retired");
    }
    let issue_table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'issues'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(issue_table_count, 0);
}

#[tokio::test]
async fn schema_18_migration_replaces_graded_judgments_with_confirmed_evidence() {
    let root = tempfile::tempdir().unwrap();
    let database_path = root.path().join("local.db");
    std::fs::File::create(&database_path).unwrap();
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    for statement in [
        "CREATE TABLE daemon_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "INSERT INTO daemon_meta (key, value) VALUES ('schema_version', '18')",
        "CREATE TABLE evaluation_cases (
            case_id TEXT PRIMARY KEY,
            source_run_id TEXT NOT NULL UNIQUE,
            corpus_id TEXT NOT NULL,
            project_id TEXT NOT NULL,
            query TEXT NOT NULL,
            query_category TEXT,
            notes TEXT,
            judgment_version BIGINT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE evaluation_judgments (
            judgment_id TEXT PRIMARY KEY,
            case_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            unit_key TEXT,
            relevance BIGINT NOT NULL,
            missed INTEGER NOT NULL,
            evidence_excerpt TEXT NOT NULL,
            notes TEXT,
            created_at TEXT NOT NULL
        )",
        "INSERT INTO evaluation_cases VALUES (
            'case_ready', 'run_ready', 'corpus_ready', 'project_test', 'ready query',
            NULL, NULL, 4, '2026-07-01T00:00:00Z', '2026-07-02T00:00:00Z'
        )",
        "INSERT INTO evaluation_cases VALUES (
            'case_draft', 'run_draft', 'corpus_draft', 'project_test', 'draft query',
            NULL, NULL, 1, '2026-07-03T00:00:00Z', '2026-07-03T00:00:00Z'
        )",
        "INSERT INTO evaluation_judgments VALUES (
            'judgment_relevant', 'case_ready', 'resource_expected', NULL, 3, 1,
            'Expected evidence', NULL, '2026-07-02T00:00:00Z'
        )",
        "INSERT INTO evaluation_judgments VALUES (
            'judgment_irrelevant', 'case_ready', 'resource_wrong', 'unit_wrong', 0, 0,
            'Wrong evidence', NULL, '2026-07-02T00:00:00Z'
        )",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let _state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        common::TestCredentialStore::default(),
    )
    .await;
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    let cases: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT case_id, status, version FROM evaluation_cases ORDER BY case_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        cases,
        vec![
            ("case_draft".to_owned(), "draft".to_owned(), 1),
            ("case_ready".to_owned(), "ready".to_owned(), 4),
        ]
    );
    let evidence: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT case_id, resource_id, unit_key FROM evaluation_evidence")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        evidence,
        vec![(
            "case_ready".to_owned(),
            "resource_expected".to_owned(),
            None
        )]
    );
    let legacy_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name = 'evaluation_judgments'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(legacy_count, 0);
}

#[test]
fn launch_agent_plist_uses_standard_identity_and_runtime_paths() {
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = "https://server.example.test".to_owned();
    let program_path = fake_daemon_program(root.path());
    let launch_agent = LaunchAgentConfig::from_daemon_config(&config, &program_path).unwrap();

    assert_eq!(IDENTIFIER_NAMESPACE, "ai.clumsies");
    assert_eq!(DAEMON_AGENT_LABEL, "ai.clumsies.daemon");
    assert_eq!(launch_agent.label, DAEMON_AGENT_LABEL);
    assert_eq!(launch_agent.mach_service_name, DAEMON_MACH_SERVICE_NAME);
    assert_eq!(
        launch_agent.plist_path,
        root.path()
            .join("LaunchAgents")
            .join(format!("{DAEMON_AGENT_LABEL}.plist"))
    );

    let plist = launch_agent.plist_contents();
    assert!(plist.contains(&format!("<string>{DAEMON_AGENT_LABEL}</string>")));
    assert!(plist.contains(&format!("<key>{DAEMON_MACH_SERVICE_NAME}</key>")));
    assert!(plist.contains("<key>MachServices</key>"));
    assert!(plist.contains("<key>CLUMSIES_DAEMON_ROOT</key>"));
    assert!(plist.contains(&format!("<string>{}</string>", root.path().display())));
    assert!(!plist.contains("CLUMSIES_DEV_INSTANCE_ID"));
    assert!(!plist.contains("CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR"));
    assert!(plist.contains(
        "<key>CLUMSIES_SERVER_URL</key>\n    <string>https://server.example.test</string>"
    ));
    assert!(!plist.contains("CODEX_HOME"));
    assert!(!plist.contains("127.0.0.1"));
    assert!(!plist.contains("daemon-endpoint.json"));

    let bootstrap = launch_agent.bootstrap_status();
    assert_eq!(bootstrap.label, DAEMON_AGENT_LABEL);
    assert_eq!(bootstrap.mach_service_name, DAEMON_MACH_SERVICE_NAME);
    assert_eq!(
        bootstrap.endpoint.transport,
        DaemonIpcTransport::MacosXpcMachService
    );
    assert_eq!(bootstrap.endpoint.service_name, DAEMON_MACH_SERVICE_NAME);
    assert!(!bootstrap.installed);
}

#[test]
fn launch_agent_install_writes_owner_only_plist() {
    let root = tempfile::tempdir().unwrap();
    let config = DaemonConfig::for_root(root.path());
    let launch_agent =
        LaunchAgentConfig::from_daemon_config(&config, fake_daemon_program(root.path())).unwrap();

    launch_agent.install_plist().unwrap();

    let plist = std::fs::read_to_string(&launch_agent.plist_path).unwrap();
    assert!(plist.contains(IDENTIFIER_NAMESPACE));
    assert!(launch_agent.root_dir.is_dir());
    assert!(launch_agent.cache_dir.is_dir());
    assert!(launch_agent.log_dir.is_dir());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = std::fs::metadata(&launch_agent.plist_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn launch_agent_controller_uses_standard_launchctl_targets() {
    let root = tempfile::tempdir().unwrap();
    let config = DaemonConfig::for_root(root.path());
    let launch_agent =
        LaunchAgentConfig::from_daemon_config(&config, fake_daemon_program(root.path())).unwrap();
    let controller = LaunchAgentController::with_domain(launch_agent, "gui/501");

    assert_eq!(controller.domain(), "gui/501");
    assert_eq!(
        controller.service_target(),
        format!("gui/501/{DAEMON_AGENT_LABEL}")
    );
    assert_eq!(
        controller.bootstrap_args(),
        vec![
            "bootstrap".to_owned(),
            "gui/501".to_owned(),
            root.path()
                .join("LaunchAgents")
                .join(format!("{DAEMON_AGENT_LABEL}.plist"))
                .display()
                .to_string(),
        ]
    );
    assert_eq!(
        controller.bootout_args(),
        vec![
            "bootout".to_owned(),
            format!("gui/501/{DAEMON_AGENT_LABEL}"),
        ]
    );
    assert_eq!(
        controller.kickstart_args(),
        vec![
            "kickstart".to_owned(),
            "-k".to_owned(),
            format!("gui/501/{DAEMON_AGENT_LABEL}"),
        ]
    );
    assert_eq!(
        controller.print_args(),
        vec!["print".to_owned(), format!("gui/501/{DAEMON_AGENT_LABEL}")],
    );
}

#[test]
fn launchctl_print_parser_reports_runtime_status() {
    let status = LaunchAgentRuntimeStatus::from_launchctl_print(
        true,
        r#"
        state = running
        pid = 12345
        last exit code = 0
        "#,
    );

    assert!(status.installed);
    assert!(status.bootstrapped);
    assert!(status.running);
    assert_eq!(status.pid, Some(12345));
    assert_eq!(status.state.as_deref(), Some("running"));
    assert_eq!(status.last_exit_code, Some(0));
    assert_eq!(status.last_error, None);
}

#[tokio::test]
async fn desktop_memory_draft_batch_is_atomic_and_preserves_existing_paths() {
    let (_root, _state, service) = common::test_daemon().await;
    let create = |path: &str| {
        json!({"create": {
            "path": path, "content": {"content": format!("# {path}")},
        }})
    };
    let batch = |operations: Vec<serde_json::Value>| {
        DaemonIpcRequest::new(
            "desktop_create_memory_drafts",
            json!({"project_id": "prj_test", "operations": operations}),
        )
    };

    let seeded = service
        .dispatch(batch(vec![create("knowledge/README.md")]))
        .await;
    assert!(seeded.ok);

    // The first insert must roll back when a later path conflicts.
    let conflict = service
        .dispatch(batch(vec![
            create("CLUMSIES.md"),
            create("knowledge/README.md"),
        ]))
        .await;
    assert!(!conflict.ok);
    let drafts = service
        .list_drafts(DaemonDraftListQuery::default())
        .await
        .unwrap();
    assert_eq!(drafts.items.len(), 1);
    assert_eq!(
        service.sync_status().await.unwrap().pending_operation_count,
        1
    );

    for operations in [
        vec![],
        vec![create("../invalid.md")],
        vec![create("same.md"), create("same.md")],
        vec![json!({"delete": {"id": "existing"}})],
    ] {
        assert!(!service.dispatch(batch(operations)).await.ok);
    }
    let completed = service
        .dispatch(batch(vec![
            create("CLUMSIES.md"),
            create("procedures/README.md"),
            create("lessons/README.md"),
        ]))
        .await;
    assert!(completed.ok, "{:?}", completed.error);
    let responses: Vec<daemon::DaemonDraftOperationResponse> = completed.into_payload().unwrap();
    assert_eq!(responses.len(), 3);
    let drafts = service
        .list_drafts(DaemonDraftListQuery::default())
        .await
        .unwrap();
    assert_eq!(drafts.items.len(), 4);
    assert_eq!(
        service.sync_status().await.unwrap().pending_operation_count,
        4
    );
    assert!(
        !service
            .dispatch(batch(vec![create("CLUMSIES.md")]))
            .await
            .ok
    );
    let original = service
        .get_draft(
            &drafts
                .items
                .iter()
                .find(|draft| draft.path.as_deref() == Some("knowledge/README.md"))
                .unwrap()
                .draft_id,
        )
        .await
        .unwrap();
    assert_eq!(original.operations.len(), 1);
}

#[tokio::test]
async fn draft_operation_is_written_to_local_queue_and_visible_in_sync_status() {
    let (_root, _state, service) = common::test_daemon().await;
    let body = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "docs/architecture.md".to_owned(),
                    content: context_content("Initial context"),
                    description: Some("seed project context".to_owned()),
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

    assert!(body.local_operation_id.starts_with("op_"));
    assert!(body.draft_id.starts_with("draft_"));
    assert!(body.queued);
    assert_eq!(body.sync_status, DraftOperationSyncStatus::Queued);

    let status = service.sync_status().await.unwrap();
    assert_eq!(status.pending_operation_count, 1);
    assert_eq!(status.failed_operation_count, 0);
    assert_eq!(status.draft_sync.state, SyncState::Queued);
    assert_eq!(status.draft_sync.server_cursor, None);
    assert_eq!(status.draft_sync.last_attempt_at, None);
    assert_eq!(status.draft_sync.last_success_at, None);
    assert_eq!(status.last_success_at, None);
}

#[tokio::test]
async fn project_sync_status_and_retry_are_isolated() {
    let (_root, state, service) = common::test_daemon().await;
    for (project_id, path) in [("prj_a", "docs/a.md"), ("prj_b", "docs/b.md")] {
        service
            .store_draft_operation(DaemonDraftOperationRequest {
                draft_id: None,
                base_commit_id: None,
                project_id: project_id.to_owned(),
                scope: DaemonDraftScope::Org,
                resource: DaemonDraftResourceKind::Memory,
                op: DaemonDraftOperation {
                    create: Some(DaemonCreateDraftOperation {
                        path: path.to_owned(),
                        content: context_content(project_id),
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
    }

    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE local_draft_operations
         SET sync_status = 'failed', last_error = 'injected failure'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let global = service.sync_status().await.unwrap();
    let project_a = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    let project_b = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_b".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(global.failed_operation_count, 2);
    assert_eq!(project_a.failed_operation_count, 1);
    assert_eq!(project_b.failed_operation_count, 1);

    service
        .project_retry_sync(DaemonProjectSyncRetryRequest {
            project_id: "prj_a".to_owned(),
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    let project_a = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    let project_b = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_b".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(project_a.pending_operation_count, 1);
    assert_eq!(project_a.failed_operation_count, 0);
    assert_eq!(project_b.pending_operation_count, 0);
    assert_eq!(project_b.failed_operation_count, 1);
}

#[tokio::test]
async fn project_draft_sync_uses_explicit_project_and_scoped_timestamps() {
    let server = FakeServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = server.url.clone();
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let service = DaemonIpcService::new(state);

    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "docs/scoped-sync.md".to_owned(),
                    content: context_content("Scoped sync"),
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

    let before = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_test".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(before.draft_sync.state, SyncState::Queued);
    assert!(before.draft_sync.last_error.is_none());

    service
        .project_retry_sync(DaemonProjectSyncRetryRequest {
            project_id: "prj_test".to_owned(),
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    let project = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_test".to_owned(),
        })
        .await
        .unwrap();
    let other_project = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_other".to_owned(),
        })
        .await
        .unwrap();
    let global = service.sync_status().await.unwrap();
    assert!(project.draft_sync.last_attempt_at.is_some());
    assert!(project.draft_sync.last_success_at.is_some());
    assert_eq!(other_project.draft_sync.last_attempt_at, None);
    assert_eq!(other_project.draft_sync.last_success_at, None);
    assert_eq!(global.draft_sync.last_attempt_at, None);
    assert_eq!(global.draft_sync.last_success_at, None);
}

#[tokio::test]
async fn draft_operation_service_method_writes_local_queue_without_http() {
    let (_root, _state, service) = common::test_daemon().await;

    let response = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "docs/ipc.md".to_owned(),
                    content: context_content("Created through daemon service"),
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

    assert!(response.local_operation_id.starts_with("op_"));
    assert!(response.draft_id.starts_with("draft_"));

    let status = service.sync_status().await.unwrap();
    assert_eq!(status.pending_operation_count, 1);
    assert_eq!(status.draft_sync.state, SyncState::Queued);

    let drafts = service
        .list_drafts(Default::default())
        .await
        .expect("service should list local drafts");
    assert_eq!(drafts.items.len(), 1);
    assert_eq!(drafts.items[0].draft_id, response.draft_id);

    let draft = service
        .get_draft(&response.draft_id)
        .await
        .expect("service should read local draft details");
    assert_eq!(draft.operations.len(), 1);
    assert_eq!(
        draft.operations[0].source,
        DaemonDraftOperationRecordSource::Desktop
    );
}

#[tokio::test]
async fn same_org_target_can_have_active_drafts_in_multiple_projects() {
    let (_root, _state, service) = common::test_daemon().await;
    let target_id = "mem_shared_org";
    let project_p = "prj_p";
    let project_q = "prj_q";

    let q_write = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: project_q.to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: target_id.to_owned(),
                        content: context_content("Q overlay"),
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

    let p_write = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: project_p.to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: target_id.to_owned(),
                        content: context_content("P overlay"),
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

    assert_ne!(q_write.draft_id, p_write.draft_id);

    let p_second_write = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: project_p.to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: target_id.to_owned(),
                        content: context_content("P overlay again"),
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

    assert_eq!(p_second_write.draft_id, p_write.draft_id);

    let q_detail = service.get_draft(&q_write.draft_id).await.unwrap();
    let p_detail = service.get_draft(&p_write.draft_id).await.unwrap();
    let q_summary = &q_detail.draft;
    let p_summary = &p_detail.draft;

    assert_eq!(q_summary.project_id, project_q);
    assert_eq!(q_summary.scope, DaemonDraftScope::Org);
    assert_eq!(q_summary.target_id.as_deref(), Some(target_id));
    assert_eq!(p_summary.project_id, project_p);
    assert_eq!(p_summary.scope, DaemonDraftScope::Org);
    assert_eq!(p_summary.target_id.as_deref(), Some(target_id));
    assert_eq!(q_detail.operations.len(), 1);
    assert_eq!(p_detail.operations.len(), 2);
}

#[tokio::test]
async fn explicit_draft_id_rejects_a_different_project_or_scope() {
    let (_root, _state, service) = common::test_daemon().await;
    let target_id = "mem_explicit_identity";
    let created = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_p".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: target_id.to_owned(),
                        content: context_content("P overlay"),
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

    let wrong_project = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(created.draft_id.clone()),
            base_commit_id: None,
            project_id: "prj_q".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: target_id.to_owned(),
                        content: context_content("Wrong project"),
                        description: None,
                    },
                )),
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await;
    assert!(matches!(wrong_project, Err(DaemonError::InvalidRequest(_))));

    let wrong_scope = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(created.draft_id),
            base_commit_id: None,
            project_id: "prj_p".to_owned(),
            scope: DaemonDraftScope::Project,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: target_id.to_owned(),
                        content: context_content("Wrong scope"),
                        description: None,
                    },
                )),
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await;
    assert!(matches!(wrong_scope, Err(DaemonError::InvalidRequest(_))));
}

#[tokio::test]
async fn deleting_a_draft_created_resource_discards_the_draft() {
    let (_root, _state, service) = common::test_daemon().await;
    let created = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "rules/new-rule.md".to_owned(),
                    content: rule_content("# New rule"),
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

    let deleted = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: None,
                rename: None,
                delete: Some(DaemonDeleteDraftOperation {
                    id: created.draft_id.clone(),
                    description: Some("remove the unpublished rule".to_owned()),
                }),
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::McpStore),
        })
        .await
        .unwrap();

    assert_eq!(deleted.draft_id, created.draft_id);
    let draft = service.get_draft(&created.draft_id).await.unwrap();
    assert_eq!(draft.draft.status, DaemonLocalDraftStatus::Discarded);
    assert_eq!(draft.operations.len(), 2);
    assert!(draft.operations[1].operation.delete.is_none());
    assert_eq!(
        draft.operations[1]
            .operation
            .discard
            .as_ref()
            .map(|operation| operation.id.as_str()),
        Some(created.draft_id.as_str())
    );
}

#[tokio::test]
async fn deleting_an_authoritative_resource_keeps_an_open_deletion_draft() {
    let (_root, _state, service) = common::test_daemon().await;
    let updated = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: "ctx_existing".to_owned(),
                        content: context_content("Updated authority"),
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
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: None,
                rename: None,
                delete: Some(DaemonDeleteDraftOperation {
                    id: "ctx_existing".to_owned(),
                    description: None,
                }),
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Desktop),
        })
        .await
        .unwrap();

    let draft = service.get_draft(&updated.draft_id).await.unwrap();
    assert_eq!(draft.draft.status, DaemonLocalDraftStatus::Open);
    assert!(draft.operations[1].operation.delete.is_some());
    assert!(draft.operations[1].operation.discard.is_none());
}

#[tokio::test]
async fn ipc_dispatch_routes_the_complete_daemon_api() {
    let (_root, _state, service) = common::test_daemon().await;

    let health: DaemonHealth = service
        .dispatch(DaemonIpcRequest::empty("health"))
        .await
        .into_payload()
        .unwrap();
    assert!(health.local_db.ready);

    let project_config = service
        .dispatch(DaemonIpcRequest::empty("project_config"))
        .await;
    assert!(project_config.ok);

    let sync_status = service
        .dispatch(DaemonIpcRequest::empty("sync_status"))
        .await;
    assert!(sync_status.ok);

    let memory_cache: DaemonMemoryCacheStatus = service
        .dispatch(DaemonIpcRequest::new(
            "memory_cache",
            serde_json::to_value(DaemonMemoryCacheRequest {
                project_id: "prj_test".to_owned(),
            })
            .unwrap(),
        ))
        .await
        .into_payload()
        .unwrap();
    assert_eq!(
        memory_cache.state,
        DaemonMemoryCacheState::ProjectRefNotSynced
    );

    let activation = service
        .dispatch(DaemonIpcRequest::new(
            "activate_memory",
            serde_json::to_value(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "writing".to_owned(),
                state: None,
            })
            .unwrap(),
        ))
        .await;
    assert!(!activation.ok);
    assert_eq!(activation.error.unwrap().code, "project_ref_not_synced");

    let project_checkout: DaemonProjectCheckout = service
        .dispatch(DaemonIpcRequest::new(
            "project_checkout",
            serde_json::to_value(DaemonProjectCheckoutRequest {
                project_id: "prj_test".to_owned(),
            })
            .unwrap(),
        ))
        .await
        .into_payload()
        .unwrap();
    assert!(!project_checkout.ready);
    assert!(project_checkout.resources.is_empty());

    let retry = service
        .dispatch(DaemonIpcRequest::new(
            "retry_sync",
            serde_json::to_value(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::All,
            })
            .unwrap(),
        ))
        .await;
    assert!(retry.ok);

    let response = service
        .dispatch(DaemonIpcRequest::new(
            "store_draft_operation",
            serde_json::to_value(DaemonDraftOperationRequest {
                draft_id: None,
                base_commit_id: None,
                project_id: "prj_test".to_owned(),
                scope: DaemonDraftScope::Org,
                resource: DaemonDraftResourceKind::Memory,
                op: DaemonDraftOperation {
                    create: Some(DaemonCreateDraftOperation {
                        path: "docs/ipc-dispatch.md".to_owned(),
                        content: context_content("Created through IPC dispatch"),
                        description: None,
                    }),
                    update: None,
                    rename: None,
                    delete: None,
                    discard: None,
                },
                source: Some(DaemonDraftOperationSource::Desktop),
            })
            .unwrap(),
        ))
        .await;
    assert!(response.ok);

    let list = service
        .dispatch(DaemonIpcRequest::new(
            "list_drafts",
            serde_json::to_value(DaemonDraftListQuery::default()).unwrap(),
        ))
        .await;
    assert!(list.ok);
    let list: daemon::DaemonDraftListResponse = list.into_payload().unwrap();
    assert_eq!(list.items.len(), 1);

    let detail = service
        .dispatch(DaemonIpcRequest::new(
            "get_draft",
            json!({ "draft_id": list.items[0].draft_id }),
        ))
        .await;
    assert!(detail.ok);

    let replaced = service
        .dispatch(DaemonIpcRequest::new(
            "replace_project_config",
            serde_json::to_value(DaemonProjectConfigUpdateRequest {
                server_url: "http://127.0.0.1:8080".to_owned(),
                project_id: Some("prj_test".to_owned()),
                memory_guidelines_path: None,
                access_token: Some("secret".to_owned()),
                refresh_token: Some("refresh-secret".to_owned()),
            })
            .unwrap(),
        ))
        .await;
    assert!(replaced.ok);

    let storage: DaemonProjectStorage = service
        .dispatch(DaemonIpcRequest::new(
            "project_storage",
            serde_json::to_value(DaemonProjectStorageRequest {
                project_id: "prj_test".to_owned(),
            })
            .unwrap(),
        ))
        .await
        .into_payload()
        .unwrap();
    assert_eq!(storage.mode, DaemonProjectStorageMode::Default);

    let cleared: DaemonProjectStorage = service
        .dispatch(DaemonIpcRequest::new(
            "clear_project_cache",
            serde_json::to_value(DaemonProjectCacheClearRequest {
                project_id: "prj_test".to_owned(),
                expected_location_revision: storage.location_revision,
            })
            .unwrap(),
        ))
        .await
        .into_payload()
        .unwrap();
    assert_eq!(cleared.location_revision, storage.location_revision);

    let status = service.sync_status().await.unwrap();
    assert_eq!(status.pending_operation_count, 1);
}

#[tokio::test]
async fn mcp_store_envelope_matches_the_daemon_contract() {
    let (_root, _state, service) = common::test_daemon().await;
    let request: DaemonIpcRequest = serde_json::from_str(
        r#"{
            "method": "store_draft_operation",
            "payload": {
                "project_id": "prj_mcp",
                "scope": "org",
                "resource": "memory",
                "op": {
                    "create": {
                        "path": "notes/from-mcp.md",
                        "content": {
                            "content": "Stored through the Zig MCP envelope"
                        }
                    }
                },
                "source": "mcp_store"
            }
        }"#,
    )
    .unwrap();

    let response = service.dispatch(request).await;
    assert!(
        response.ok,
        "daemon rejected MCP envelope: {:?}",
        response.error
    );

    let drafts = service
        .list_drafts(DaemonDraftListQuery::default())
        .await
        .unwrap();
    assert_eq!(drafts.items.len(), 1);
    assert_eq!(drafts.items[0].project_id, "prj_mcp");
    assert_eq!(drafts.items[0].scope, DaemonDraftScope::Org);

    let detail = service.get_draft(&drafts.items[0].draft_id).await.unwrap();
    assert_eq!(detail.operations.len(), 1);
    assert_eq!(
        detail.operations[0].source,
        DaemonDraftOperationRecordSource::McpStore
    );
}

#[tokio::test]
async fn local_drafts_can_be_listed_and_read_with_operation_history() {
    let (_root, _state, service) = common::test_daemon().await;

    let created = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "docs/local.md".to_owned(),
                    content: context_content("Local draft"),
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

    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(created.draft_id.clone()),
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: Some(DaemonUpdateDraftOperation::Content(
                    DaemonContentDraftUpdate {
                        id: created.draft_id.clone(),
                        content: context_content("Local draft v2"),
                        description: None,
                    },
                )),
                rename: None,
                delete: None,
                discard: None,
            },
            source: Some(DaemonDraftOperationSource::Cli),
        })
        .await
        .unwrap();

    let list = service
        .list_drafts(DaemonDraftListQuery {
            resource: Some("memory".to_owned()),
            status: Some("open".to_owned()),
            limit: None,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(list.items.len(), 1);
    let item = &list.items[0];
    assert_eq!(item.draft_id, created.draft_id);
    assert_eq!(item.server_draft_id, None);
    assert_eq!(item.server_version, 0);
    assert_eq!(item.resource_kind, DaemonDraftResourceKind::Memory);
    assert_eq!(item.path.as_deref(), Some("docs/local.md"));
    assert_eq!(item.status, DaemonLocalDraftStatus::Open);
    assert_eq!(item.pending_operation_count, 2);
    assert_eq!(item.failed_operation_count, 0);

    let detail = service.get_draft(&created.draft_id).await.unwrap();
    assert_eq!(detail.draft.draft_id, created.draft_id);
    assert_eq!(detail.operations.len(), 2);
    assert_eq!(
        detail.operations[0].source,
        DaemonDraftOperationRecordSource::McpStore
    );
    assert_eq!(
        detail.operations[0].sync_status,
        DraftOperationSyncStatus::Queued
    );
    assert_eq!(
        detail.operations[0]
            .operation
            .create
            .as_ref()
            .unwrap()
            .content,
        context_content("Local draft")
    );
    assert_eq!(
        detail.operations[1].source,
        DaemonDraftOperationRecordSource::Cli
    );
    assert_eq!(
        detail.operations[1]
            .operation
            .update
            .as_ref()
            .unwrap()
            .content(),
        Some(&context_content("Local draft v2"))
    );

    assert!(matches!(
        service.get_draft("missing").await,
        Err(DaemonError::NotFound(_))
    ));
}

#[tokio::test]
async fn local_draft_inventory_paginates_past_terminal_history() {
    let (_root, state, service) = common::test_daemon().await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    sqlx::query(
        "WITH RECURSIVE sequence(value) AS (
            VALUES(0)
            UNION ALL
            SELECT value + 1 FROM sequence WHERE value < 501
         )
         INSERT INTO local_drafts (
            draft_id, project_id, resource_scope, resource_kind, status, created_at, updated_at
         )
         SELECT
            printf('draft-%03d', value),
            'prj_test',
            'org',
            'memory',
            CASE
                WHEN value < 500 AND value % 2 = 0 THEN 'discarded'
                WHEN value < 500 THEN 'merged'
                WHEN value = 500 THEN 'open'
                ELSE 'submitted'
            END,
            CASE
                WHEN value < 500 THEN '2026-08-26T12:00:00Z'
                ELSE '2026-08-25T12:00:00Z'
            END,
            CASE
                WHEN value < 500 THEN '2026-08-26T12:00:00Z'
                ELSE '2026-08-25T12:00:00Z'
            END
         FROM sequence",
    )
    .execute(&pool)
    .await
    .unwrap();

    let first = service
        .list_drafts(DaemonDraftListQuery {
            resource: None,
            status: None,
            cursor: None,
            limit: Some(500),
        })
        .await
        .unwrap();
    assert_eq!(first.items.len(), 500);
    assert!(first.items.iter().all(|draft| matches!(
        draft.status,
        DaemonLocalDraftStatus::Discarded | DaemonLocalDraftStatus::Merged
    )));
    let cursor = first
        .next_cursor
        .expect("the full first page must expose a cursor");

    let second = service
        .list_drafts(DaemonDraftListQuery {
            resource: None,
            status: None,
            cursor: Some(cursor),
            limit: Some(500),
        })
        .await
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|draft| draft.draft_id.as_str())
            .collect::<Vec<_>>(),
        ["draft-500", "draft-501"]
    );
    assert_eq!(
        second
            .items
            .iter()
            .map(|draft| draft.status)
            .collect::<Vec<_>>(),
        [
            DaemonLocalDraftStatus::Open,
            DaemonLocalDraftStatus::Submitted
        ]
    );
    assert!(second.next_cursor.is_none());

    let invalid = service
        .list_drafts(DaemonDraftListQuery {
            resource: None,
            status: None,
            cursor: Some("not-a-cursor".to_owned()),
            limit: Some(500),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        invalid,
        DaemonError::InvalidRequest(message) if message == "Invalid Draft list cursor"
    ));
}

#[tokio::test]
async fn project_config_can_be_replaced_and_persists_across_restarts() {
    let root = tempfile::tempdir().unwrap();
    let credential_store = common::TestCredentialStore::default();
    let state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        credential_store.clone(),
    )
    .await;
    let service = DaemonIpcService::new(state);

    let initial = service.project_config();
    assert_eq!(initial.server_url, "");
    assert_eq!(initial.project_id, None);
    assert!(!initial.has_access_token);
    assert!(!initial.ready);
    assert_eq!(
        initial.missing_fields,
        vec!["server_url", "project_id", "access_token"]
    );

    let updated = service
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: "http://127.0.0.1:18080".to_owned(),
            project_id: Some("prj_config".to_owned()),
            memory_guidelines_path: None,
            access_token: Some("secret-token".to_owned()),
            refresh_token: Some("refresh-token".to_owned()),
        })
        .await
        .unwrap();
    assert_eq!(updated.server_url, "http://127.0.0.1:18080");
    assert_eq!(updated.project_id.as_deref(), Some("prj_config"));
    assert!(updated.has_access_token);
    assert!(updated.has_refresh_token);
    assert!(updated.ready);
    assert!(updated.missing_fields.is_empty());
    let stored_credentials = credential_store.credentials().unwrap();
    assert_eq!(stored_credentials.server_url, "http://127.0.0.1:18080");
    assert_eq!(stored_credentials.access_token, "secret-token");
    assert_eq!(
        stored_credentials.refresh_token.as_deref(),
        Some("refresh-token")
    );

    let metadata_pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        root.path().join("local.db").display()
    ))
    .await
    .unwrap();
    let persisted_secret_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM daemon_meta
         WHERE key IN ('project_config_access_token', 'project_config_refresh_token')",
    )
    .fetch_one(&metadata_pool)
    .await
    .unwrap();
    assert_eq!(persisted_secret_count, 0);
    metadata_pool.close().await;

    let health = service.health().await;
    assert_eq!(health.server_url, "http://127.0.0.1:18080");
    assert_eq!(health.project_id.as_deref(), Some("prj_config"));

    let restarted = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        credential_store.clone(),
    )
    .await;
    let restarted_service = DaemonIpcService::new(restarted);
    let persisted = restarted_service.project_config();
    assert_eq!(persisted.server_url, "http://127.0.0.1:18080");
    assert_eq!(persisted.project_id.as_deref(), Some("prj_config"));
    assert!(persisted.has_access_token);
    assert!(persisted.has_refresh_token);
    assert!(persisted.ready);

    let signed_out = restarted_service
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: "http://127.0.0.1:18080".to_owned(),
            project_id: Some("prj_config".to_owned()),
            memory_guidelines_path: None,
            access_token: None,
            refresh_token: None,
        })
        .await
        .unwrap();
    assert!(!signed_out.has_access_token);
    assert!(!signed_out.has_refresh_token);
    assert!(credential_store.credentials().is_none());
}

#[tokio::test]
async fn project_config_supports_custom_memory_guidelines_path() {
    let root = tempfile::tempdir().unwrap();
    let credential_store = common::TestCredentialStore::default();
    let state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        credential_store.clone(),
    )
    .await;
    let service = DaemonIpcService::new(state);

    let updated = service
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: "http://127.0.0.1:18080".to_owned(),
            project_id: Some("prj_custom".to_owned()),
            memory_guidelines_path: Some("./README.md".to_owned()),
            access_token: Some("secret-token".to_owned()),
            refresh_token: None,
        })
        .await
        .unwrap();
    assert_eq!(
        updated.memory_guidelines_path.as_deref(),
        Some("./README.md")
    );

    let restarted = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        credential_store.clone(),
    )
    .await;
    let restarted_service = DaemonIpcService::new(restarted);
    let persisted = restarted_service.project_config();
    assert_eq!(
        persisted.memory_guidelines_path.as_deref(),
        Some("./README.md")
    );
}

#[tokio::test]
async fn selecting_a_project_preserves_credentials_and_persists_the_active_project() {
    let root = tempfile::tempdir().unwrap();
    let credential_store = common::TestCredentialStore::default();
    let state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        credential_store.clone(),
    )
    .await;
    let service = DaemonIpcService::new(state);

    service
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: "https://clumsies.example.com".to_owned(),
            project_id: Some("prj_first".to_owned()),
            memory_guidelines_path: None,
            access_token: Some("access-token".to_owned()),
            refresh_token: Some("refresh-token".to_owned()),
        })
        .await
        .unwrap();

    let selected = service
        .select_project(DaemonProjectSelectionRequest {
            project_id: "prj_second".to_owned(),
        })
        .await
        .unwrap();

    assert_eq!(selected.project_id.as_deref(), Some("prj_second"));
    assert_eq!(selected.server_url, "https://clumsies.example.com");
    assert!(selected.has_access_token);
    assert!(selected.has_refresh_token);
    assert_eq!(
        credential_store.credentials().unwrap(),
        ServerCredentials {
            server_url: "https://clumsies.example.com".to_owned(),
            access_token: "access-token".to_owned(),
            refresh_token: Some("refresh-token".to_owned()),
        }
    );

    let restarted = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        credential_store.clone(),
    )
    .await;
    assert_eq!(
        restarted.project_config_status().project_id.as_deref(),
        Some("prj_second")
    );

    let error = DaemonIpcService::new(restarted)
        .select_project(DaemonProjectSelectionRequest {
            project_id: "  ".to_owned(),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("project_id must not be empty"));
}

fn identity_response(org_id: &str, projects: &[&str]) -> Json<serde_json::Value> {
    Json(
        json!({"org": {"org_id": org_id}, "projects": projects.iter().map(|id| json!({"project_id": id})).collect::<Vec<_>>() }),
    )
}

async fn bindings_identity() -> Json<serde_json::Value> {
    identity_response(
        "org_bindings",
        &[
            "prj_a",
            "prj_b",
            "prj_bound_first",
            "prj_bound_second",
            "prj_desktop_selection",
        ],
    )
}

async fn fake_identity() -> Json<serde_json::Value> {
    identity_response(
        "org_test",
        &[
            "prj_test",
            "prj_recovery",
            "prj_late",
            "prj_retrying",
            "prj_retry_lock",
            "prj_terminal",
            "prj_conflict",
        ],
    )
}

async fn atomic_identity() -> Json<serde_json::Value> {
    identity_response("org_atomic", &["prj_atomic"])
}

#[derive(Clone, Default)]
struct ProjectLifecycleProbe {
    membership: Arc<AtomicUsize>,
    commit_status: Arc<AtomicUsize>,
    project_requests: Arc<Mutex<Vec<String>>>,
    org_requests: Arc<AtomicUsize>,
    uploads: Arc<AtomicUsize>,
    send_events: Arc<AtomicBool>,
    projections: Arc<AtomicUsize>,
}

async fn lifecycle_events(
    axum::extract::State(probe): axum::extract::State<ProjectLifecycleProbe>,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
) -> Json<serde_json::Value> {
    if !probe.send_events.load(Ordering::SeqCst) || query.contains_key("after_cursor") {
        return empty_draft_events().await;
    }
    Json(json!({"events": [{
        "event_id": "evt_retained", "draft_id": "drf_remote", "project_id": "prj_recovery",
        "event_type": "discarded", "version": 1, "daemon_installation_id": null,
        "created_at": "2026-09-22T00:00:00Z"
    }], "next_cursor": "1", "has_more": false}))
}

async fn lifecycle_projection(
    axum::extract::State(probe): axum::extract::State<ProjectLifecycleProbe>,
) -> Json<serde_json::Value> {
    probe.projections.fetch_add(1, Ordering::SeqCst);
    assert_eq!(
        probe.membership.load(Ordering::SeqCst),
        1,
        "Inaccessible Drafts must not be fetched"
    );
    Json(json!({"draft": {
        "draft_id": "drf_remote", "project_id": "prj_recovery", "base_commit_id": null,
        "resource": {"scope": "org", "id": null, "path": "context/remote.md"},
        "coordination": {"current_commit_id": null, "freshness": "current",
            "has_upstream_resource_changes": false, "reconciliation": "unknown", "candidate_id": null},
        "status": "discarded", "version": 1, "created_at": "2026-09-22T00:00:00Z", "updated_at": "2026-09-22T00:00:00Z"
    }, "operations": []}))
}

async fn lifecycle_identity(
    axum::extract::State(probe): axum::extract::State<ProjectLifecycleProbe>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match probe.membership.load(Ordering::SeqCst) {
        0 => identity_response("org_test", &["prj_test"]).into_response(),
        1 => identity_response("org_test", &["prj_test", "prj_recovery"]).into_response(),
        2 => identity_response("org_test", &[]).into_response(),
        3 => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "identity unavailable",
        )
            .into_response(),
        4 => Json(json!({"org": {"org_id": "org_test"}})).into_response(),
        _ => (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
    }
}

async fn lifecycle_project_commit(
    axum::extract::State(probe): axum::extract::State<ProjectLifecycleProbe>,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    probe
        .project_requests
        .lock()
        .unwrap()
        .push(project_id.clone());
    match probe.commit_status.load(Ordering::SeqCst) {
        0 => fake_project_commit_state(axum::extract::Path(project_id))
            .await
            .into_response(),
        200 => Json(json!({})).into_response(), // Successful protocol response really lacks ETag.
        code => (
            axum::http::StatusCode::from_u16(code as u16).unwrap(),
            "project unavailable",
        )
            .into_response(),
    }
}

async fn lifecycle_org_commit(
    axum::extract::State(probe): axum::extract::State<ProjectLifecycleProbe>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    probe.org_requests.fetch_add(1, Ordering::SeqCst);
    fake_org_commit_state().await.into_response()
}

async fn lifecycle_upload(
    axum::extract::State(probe): axum::extract::State<ProjectLifecycleProbe>,
) -> Json<serde_json::Value> {
    probe.uploads.fetch_add(1, Ordering::SeqCst);
    Json(json!({"draft": {"draft_id": "drf_restored", "version": 1}}))
}

async fn lifecycle_server(probe: ProjectLifecycleProbe) -> String {
    let app = Router::new()
        .route("/api/v1/me", get(lifecycle_identity))
        .route("/api/v1/projects/{project_id}", get(accessible_project))
        .route(
            "/api/v1/projects/{project_id}/commit-state",
            get(lifecycle_project_commit),
        )
        .route("/api/v1/org/commit-state", get(lifecycle_org_commit))
        .route("/api/v1/draft-events", get(lifecycle_events))
        .route("/api/v1/drafts", post(lifecycle_upload))
        .route("/api/v1/drafts/{draft_id}", get(lifecycle_projection))
        .with_state(probe);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

#[tokio::test]
async fn unavailable_projects_pause_without_losing_local_work_and_resume_after_access_returns() {
    let probe = ProjectLifecycleProbe::default();
    probe.send_events.store(true, Ordering::SeqCst);
    let root = tempfile::tempdir().unwrap();
    let workspaces = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = lifecycle_server(probe.clone()).await;
    config.project.project_id = Some("prj_recovery".to_owned());
    let (state, credentials) =
        common::initialize_authenticated_daemon(config.clone(), "test-token", None).await;
    for id in ["prj_test", "prj_recovery"] {
        let path = workspaces.path().join(id);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("user-file.txt"), "keep me").unwrap();
        state
            .replace_project_binding(DaemonProjectBindingReplaceRequest {
                workspace_root: path.display().to_string(),
                project_id: id.to_owned(),
                expected_revision: None,
            })
            .await
            .unwrap();
    }
    let stored = state
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_recovery".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/retained.md".to_owned(),
                    content: context_content("Retain this proposal"),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: None,
        })
        .await
        .unwrap();
    // Seed exactly the diagnostic left by old clients; a successful cycle must retire it.
    let pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    sqlx::query("INSERT INTO daemon_meta(key,value) VALUES ('commit_sync_last_error','Commit state response is missing ETag')").execute(&pool).await.unwrap();
    pool.close().await;
    for _ in 0..2 {
        state
            .retry_sync(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::All,
            })
            .await
            .unwrap();
        let status = state.sync_status().await.unwrap();
        assert_eq!(status.pending_operation_count, 0);
        assert_eq!(status.failed_operation_count, 0);
        assert_eq!(status.commit_sync.state, SyncState::Idle);
        assert!(status.commit_sync.last_error.is_none());
        assert_eq!(status.unavailable_projects.len(), 1);
        assert_eq!(status.unavailable_projects[0].project_id, "prj_recovery");
        assert_eq!(status.unavailable_projects[0].draft_count, 1);
        assert_eq!(status.unavailable_projects[0].bindings.len(), 1);
        let draft = state.get_draft(&stored.draft_id).await.unwrap();
        assert_eq!(
            draft.operations[0].sync_status,
            DraftOperationSyncStatus::Queued
        );
        assert_eq!(
            draft.operations[0]
                .operation
                .create
                .as_ref()
                .unwrap()
                .content,
            context_content("Retain this proposal")
        );
    }
    assert_eq!(probe.uploads.load(Ordering::SeqCst), 0);
    assert_eq!(probe.projections.load(Ordering::SeqCst), 0);
    assert_eq!(
        state
            .sync_status()
            .await
            .unwrap()
            .draft_sync
            .server_cursor
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        *probe.project_requests.lock().unwrap(),
        ["prj_test", "prj_test"]
    );
    assert_eq!(state.project_config_status().project_id, None);
    let request = || DaemonProjectBindingResolveRequest {
        workspace_path: workspaces.path().join("prj_recovery").display().to_string(),
        required_adapter: None,
    };
    assert!(matches!(
        state.resolve_project_binding(request()).await.unwrap_err(),
        DaemonError::State {
            code: "project_binding_unresolved",
            ..
        }
    ));
    drop(state);
    let state = common::initialize_daemon(config, credentials).await;
    state
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::All,
        })
        .await
        .unwrap();
    assert_eq!(
        state.sync_status().await.unwrap().unavailable_projects[0].draft_count,
        1
    );
    probe.membership.store(1, Ordering::SeqCst);
    state
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::All,
        })
        .await
        .unwrap();
    assert!(
        state
            .sync_status()
            .await
            .unwrap()
            .unavailable_projects
            .is_empty()
    );
    assert_eq!(probe.uploads.load(Ordering::SeqCst), 1);
    assert_eq!(
        probe.projections.load(Ordering::SeqCst),
        1,
        "Deferred events must replay after restored access and restart"
    );
    assert_eq!(
        state.get_draft(&stored.draft_id).await.unwrap().operations[0].sync_status,
        DraftOperationSyncStatus::Synced
    );
    assert_eq!(
        state
            .resolve_project_binding(request())
            .await
            .unwrap()
            .project_id,
        "prj_recovery"
    );

    // Removing the final projects still syncs the organization ref.
    probe.membership.store(2, Ordering::SeqCst);
    let before_projects = probe.project_requests.lock().unwrap().len();
    let before_org = probe.org_requests.load(Ordering::SeqCst);
    state
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::All,
        })
        .await
        .unwrap();
    assert_eq!(
        probe.project_requests.lock().unwrap().len(),
        before_projects
    );
    assert_eq!(probe.org_requests.load(Ordering::SeqCst), before_org + 1);
    let unavailable = state.sync_status().await.unwrap().unavailable_projects;
    assert_eq!(unavailable.len(), 2);
    let binding = &unavailable
        .iter()
        .find(|p| p.project_id == "prj_test")
        .unwrap()
        .bindings[0];
    let moved = workspaces.path().join("moved-test");
    std::fs::rename(&binding.workspace_root, &moved).unwrap();
    assert!(
        state
            .remove_project_binding(DaemonProjectBindingRemoveRequest {
                workspace_root: binding.workspace_root.clone(),
                expected_revision: binding.revision + 1,
            })
            .await
            .is_err()
    );
    state
        .remove_project_binding(DaemonProjectBindingRemoveRequest {
            workspace_root: binding.workspace_root.clone(),
            expected_revision: binding.revision,
        })
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(moved.join("user-file.txt")).unwrap(),
        "keep me"
    );
    assert_eq!(
        state
            .sync_status()
            .await
            .unwrap()
            .unavailable_projects
            .len(),
        1
    );
}

#[tokio::test]
async fn failed_membership_reads_preserve_bindings_and_http_errors_precede_etag_validation() {
    let probe = ProjectLifecycleProbe::default();
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = lifecycle_server(probe.clone()).await;
    config.project.project_id = Some("prj_test".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    state
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: workspace.path().display().to_string(),
            project_id: "prj_test".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();
    for code in [404, 401, 403, 500, 200, 0] {
        probe.commit_status.store(code, Ordering::SeqCst);
        let result = state
            .retry_sync(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::Commits,
            })
            .await;
        if code == 0 {
            result.unwrap();
        } else {
            let error = result.unwrap_err().to_string();
            assert_eq!(
                error.contains("missing ETag"),
                code == 200,
                "{code}: {error}"
            );
            if code != 200 {
                assert!(error.contains(&code.to_string()), "{error}");
            }
        }
        // A per-project 404 is insufficient to discard a binding or stop future syncs.
        assert!(
            state
                .sync_status()
                .await
                .unwrap()
                .unavailable_projects
                .is_empty()
        );
    }
    let before = probe.project_requests.lock().unwrap().len();
    for membership in [3, 4, 5] {
        probe.membership.store(membership, Ordering::SeqCst);
        assert!(
            state
                .retry_sync(DaemonSyncRetryRequest {
                    channel: SyncRetryChannel::All
                })
                .await
                .is_err()
        );
        assert_eq!(
            state.project_config_status().project_id.as_deref(),
            Some("prj_test")
        );
        assert!(
            state
                .sync_status()
                .await
                .unwrap()
                .unavailable_projects
                .is_empty()
        );
        assert_eq!(probe.project_requests.lock().unwrap().len(), before);
    }
    assert_eq!(
        state
            .list_project_bindings(DaemonProjectBindingListRequest {
                project_id: "prj_test".to_owned()
            })
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn membership_from_a_previous_session_cannot_clear_the_new_selection() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let app = Router::new().route(
        "/api/v1/me",
        get({
            let started = started.clone();
            let release = release.clone();
            move || {
                let started = started.clone();
                let release = release.clone();
                async move {
                    started.notify_one();
                    release.notified().await;
                    identity_response("org_test", &[])
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_test".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let pending_state = state.clone();
    let pending = tokio::spawn(async move {
        pending_state
            .retry_sync(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::All,
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    state
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: format!("http://{address}"),
            project_id: Some("prj_new_session".to_owned()),
            memory_guidelines_path: None,
            access_token: Some("replacement-token".to_owned()),
            refresh_token: None,
        })
        .await
        .unwrap();
    release.notify_one();
    assert!(matches!(
        pending.await.unwrap().unwrap_err(),
        DaemonError::State {
            code: "sync_session_changed",
            ..
        }
    ));
    assert_eq!(
        state.project_config_status().project_id.as_deref(),
        Some("prj_new_session")
    );
    assert!(
        state
            .sync_status()
            .await
            .unwrap()
            .unavailable_projects
            .is_empty()
    );
}

async fn accessible_project() -> Json<serde_json::Value> {
    Json(json!({
        "project_id": "accessible",
        "name": "Accessible Project"
    }))
}

async fn empty_project_commit_state(
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::ETAG, "\"ref-none\"")],
        Json(json!({
            "update_available": false,
            "ref": {
                "name": "refs/heads/main",
                "scope": "project",
                "org_id": "org_bindings",
                "project_id": project_id,
                "commit_id": null,
                "updated_at": "2026-07-22T00:00:00Z"
            },
            "latest": null,
            "download_url": null,
            "incremental_supported": false
        })),
    )
}

async fn empty_org_commit_state() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::ETAG, "\"ref-none\"")],
        Json(json!({
            "update_available": false,
            "ref": {
                "name": "refs/heads/main",
                "scope": "org",
                "org_id": "org_bindings",
                "project_id": null,
                "commit_id": null,
                "updated_at": "2026-07-22T00:00:00Z"
            },
            "latest": null,
            "download_url": null,
            "incremental_supported": false
        })),
    )
}

async fn failing_a_empty_b_project_commit_state(
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if project_id == "prj_a" {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "injected project A failure" })),
        )
            .into_response();
    }
    empty_project_commit_state(axum::extract::Path(project_id))
        .await
        .into_response()
}

#[derive(Clone)]
struct CommitSyncProbeState {
    org_requests: Arc<AtomicUsize>,
    project_requests: Arc<AtomicUsize>,
    project_in_flight: Arc<AtomicUsize>,
    max_project_in_flight: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct ScopedCommitSyncProbeState {
    project_requests: Arc<Mutex<Vec<String>>>,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[derive(Deserialize)]
struct ScopedCommitStateQuery {
    local_commit_id: Option<String>,
}

async fn blocking_scoped_project_commit_state(
    axum::extract::State(state): axum::extract::State<ScopedCommitSyncProbeState>,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<ScopedCommitStateQuery>,
) -> impl axum::response::IntoResponse {
    state
        .project_requests
        .lock()
        .unwrap()
        .push(project_id.clone());
    state.started.notify_one();
    state.release.notified().await;
    (
        [(axum::http::header::ETAG, "\"ref-none\"")],
        Json(json!({
            "update_available": query.local_commit_id.is_some(),
            "ref": {
                "name": "refs/heads/main",
                "scope": "project",
                "org_id": "org_bindings",
                "project_id": project_id,
                "commit_id": null,
                "updated_at": "2026-08-27T00:00:00Z"
            },
            "latest": null,
            "download_url": null,
            "incremental_supported": false
        })),
    )
}

async fn probed_empty_project_commit_state(
    axum::extract::State(state): axum::extract::State<CommitSyncProbeState>,
    project_id: axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    state.project_requests.fetch_add(1, Ordering::AcqRel);
    let in_flight = state.project_in_flight.fetch_add(1, Ordering::AcqRel) + 1;
    state
        .max_project_in_flight
        .fetch_max(in_flight, Ordering::AcqRel);
    tokio::time::sleep(Duration::from_millis(40)).await;
    state.project_in_flight.fetch_sub(1, Ordering::AcqRel);
    empty_project_commit_state(project_id).await
}

async fn probed_empty_org_commit_state(
    axum::extract::State(state): axum::extract::State<CommitSyncProbeState>,
) -> impl axum::response::IntoResponse {
    state.org_requests.fetch_add(1, Ordering::AcqRel);
    empty_org_commit_state().await
}

#[tokio::test]
async fn project_bindings_resolve_by_canonical_root_and_persist_across_restarts() {
    let app = Router::new().route("/api/v1/projects/{project_id}", get(accessible_project));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let daemon_root = tempfile::tempdir().unwrap();
    let workspaces = tempfile::tempdir().unwrap();
    let first_root = workspaces.path().join("first");
    let first_nested = first_root.join("packages/app");
    let second_root = workspaces.path().join("second");
    std::fs::create_dir_all(&first_nested).unwrap();
    std::fs::create_dir_all(&second_root).unwrap();

    let mut config = DaemonConfig::for_root(daemon_root.path());
    config.project.server_url = format!("http://{address}/");
    let credential_store = common::TestCredentialStore::new(Some(ServerCredentials {
        server_url: config.project.server_url.clone(),
        access_token: "test-token".to_owned(),
        refresh_token: None,
    }));
    let state = common::initialize_daemon(config.clone(), credential_store.clone()).await;
    let service = DaemonIpcService::new(state);

    let first = service
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: first_root.display().to_string(),
            project_id: "prj_first".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.server_url, format!("http://{address}"));

    let second = service
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: second_root.display().to_string(),
            project_id: "prj_second".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();
    assert_eq!(second.revision, 1);
    let first_project_bindings = service
        .list_project_bindings(DaemonProjectBindingListRequest {
            project_id: "prj_first".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(first_project_bindings.items, vec![first.clone()]);

    let first_resolution = service.resolve_project_binding(DaemonProjectBindingResolveRequest {
        workspace_path: first_nested.display().to_string(),
        required_adapter: None,
    });
    let second_resolution = service.resolve_project_binding(DaemonProjectBindingResolveRequest {
        workspace_path: second_root.display().to_string(),
        required_adapter: None,
    });
    let (first_resolution, second_resolution) = tokio::join!(first_resolution, second_resolution);
    assert_eq!(first_resolution.unwrap().project_id, "prj_first");
    assert_eq!(second_resolution.unwrap().project_id, "prj_second");

    let restarted = common::initialize_daemon(config, credential_store).await;
    let persisted = restarted
        .resolve_project_binding(DaemonProjectBindingResolveRequest {
            workspace_path: first_nested.display().to_string(),
            required_adapter: None,
        })
        .await
        .unwrap();
    assert_eq!(persisted.project_id, "prj_first");
    assert_eq!(
        persisted.workspace_root,
        std::fs::canonicalize(&first_root)
            .unwrap()
            .display()
            .to_string()
    );

    let error = restarted
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: first_root.display().to_string(),
            project_id: "prj_other".to_owned(),
            expected_revision: Some(99),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        DaemonError::State {
            code: "project_binding_changed",
            ..
        }
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn project_bindings_resolve_when_the_workspace_root_is_replaced_by_a_symlink() {
    let app = Router::new().route("/api/v1/projects/{project_id}", get(accessible_project));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let daemon_root = tempfile::tempdir().unwrap();
    let workspaces = tempfile::tempdir().unwrap();
    let moved_volume = workspaces.path().join("volume");
    std::fs::create_dir_all(moved_volume.join("clumsies")).unwrap();
    let bound_root = workspaces.path().join("workspace").join("clumsies");
    std::fs::create_dir_all(&bound_root).unwrap();

    let mut config = DaemonConfig::for_root(daemon_root.path());
    config.project.server_url = format!("http://{address}/");
    let credential_store = common::TestCredentialStore::new(Some(ServerCredentials {
        server_url: config.project.server_url.clone(),
        access_token: "test-token".to_owned(),
        refresh_token: None,
    }));
    let state = common::initialize_daemon(config, credential_store).await;
    let service = DaemonIpcService::new(state);

    service
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: bound_root.display().to_string(),
            project_id: "prj_moved".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();

    // Simulate a repository migration: the old parent directory is replaced
    // by a symlink to the new volume, so the bound root now canonicalizes
    // to a different location than the one stored at bind time.
    std::fs::remove_dir_all(workspaces.path().join("workspace")).unwrap();
    std::os::unix::fs::symlink(&moved_volume, workspaces.path().join("workspace")).unwrap();

    let resolved = service
        .resolve_project_binding(DaemonProjectBindingResolveRequest {
            workspace_path: bound_root.display().to_string(),
            required_adapter: None,
        })
        .await
        .unwrap();
    assert_eq!(resolved.project_id, "prj_moved");
}

#[cfg(unix)]
#[tokio::test]
async fn adapter_install_normalizes_an_empty_binding_after_a_workspace_symlink_move() {
    let app = Router::new().route("/api/v1/projects/{project_id}", get(accessible_project));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let daemon_root = tempfile::tempdir().unwrap();
    let workspaces = tempfile::tempdir().unwrap();
    let moved_volume = workspaces.path().join("volume");
    let canonical_root = moved_volume.join("repository");
    std::fs::create_dir_all(&canonical_root).unwrap();
    let alias_parent = workspaces.path().join("workspace");
    let bound_root = alias_parent.join("repository");
    std::fs::create_dir_all(&bound_root).unwrap();

    let mut config = DaemonConfig::for_root(daemon_root.path());
    config.project.server_url = format!("http://{address}/");
    let credential_store = common::TestCredentialStore::new(Some(ServerCredentials {
        server_url: config.project.server_url.clone(),
        access_token: "test-token".to_owned(),
        refresh_token: None,
    }));
    let state = common::initialize_daemon(config, credential_store).await;
    let service = DaemonIpcService::new(state);
    service
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: bound_root.display().to_string(),
            project_id: "prj_moved_adapter".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();

    std::fs::remove_dir_all(&alias_parent).unwrap();
    std::os::unix::fs::symlink(&moved_volume, &alias_parent).unwrap();

    let helper = signed_runtime_binary(daemon_root.path());
    let installed = service
        .install_project_agent_adapter(DaemonProjectAgentAdapterInstallRequest {
            project_id: "prj_moved_adapter".to_owned(),
            workspace_root: bound_root.display().to_string(),
            adapter: ProjectAgentAdapterKind::ClaudeCode,
            runtime_binary_path: helper.display().to_string(),
            host_binary_path: None,
            expected_revision: None,
        })
        .await
        .unwrap();
    let canonical_root = std::fs::canonicalize(&canonical_root).unwrap();
    assert_eq!(
        installed.workspace_root,
        canonical_root.display().to_string()
    );

    let bindings = service
        .list_project_bindings(DaemonProjectBindingListRequest {
            project_id: "prj_moved_adapter".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(bindings.items.len(), 1);
    assert_eq!(
        bindings.items[0].workspace_root,
        canonical_root.display().to_string()
    );

    service
        .remove_project_agent_adapter(DaemonProjectAgentAdapterRemoveRequest {
            workspace_root: bound_root.display().to_string(),
            adapter: ProjectAgentAdapterKind::ClaudeCode,
            expected_revision: installed.revision,
        })
        .await
        .unwrap();
    let removed = service
        .remove_project_binding(DaemonProjectBindingRemoveRequest {
            workspace_root: bound_root.display().to_string(),
            expected_revision: bindings.items[0].revision,
        })
        .await
        .unwrap();
    assert!(removed.removed);
}

#[tokio::test]
async fn global_adapter_migrates_repository_files_and_persists_disabled_choice_offline() {
    let root = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.dev_instance_id = Some("global-adapter-test".to_owned());
    config.project.server_url = "https://clumsies.example.test".to_owned();
    let state =
        common::initialize_daemon(config.clone(), common::TestCredentialStore::default()).await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    let workspace = std::fs::canonicalize(repo.path())
        .unwrap()
        .display()
        .to_string();
    sqlx::query("INSERT INTO project_bindings (server_url, workspace_root, project_id, revision) VALUES ($1, $2, 'prj_adapter', 1)")
        .bind(state.project_config_status().server_url).bind(&workspace).execute(&pool).await.unwrap();
    std::fs::write(
        repo.path().join("opencode.json"),
        r#"{"model":"user-model"}"#,
    )
    .unwrap();
    let runtime = signed_runtime_binary(root.path());
    let old_request = DaemonProjectAgentAdapterInstallRequest {
        project_id: "prj_adapter".to_owned(),
        workspace_root: workspace.clone(),
        adapter: ProjectAgentAdapterKind::Opencode,
        runtime_binary_path: runtime.display().to_string(),
        host_binary_path: None,
        expected_revision: None,
    };
    state
        .install_project_agent_adapter(old_request.clone())
        .await
        .unwrap();
    let mut request = daemon::DaemonSetAgentAdapterRequest {
        adapter: ProjectAgentAdapterKind::Opencode,
        enabled: true,
        runtime_binary_path: runtime.display().to_string(),
        host_binary_path: None,
    };
    let settings = state.set_agent_adapter(request.clone()).await.unwrap();
    let selected = settings
        .items
        .iter()
        .find(|item| item.adapter == request.adapter)
        .unwrap();
    assert!(selected.enabled && selected.installed && selected.configured);
    assert_eq!(selected.legacy_repositories, 0);
    let repository_config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(repo.path().join("opencode.json")).unwrap()).unwrap();
    assert_eq!(repository_config, json!({"model": "user-model"}));
    assert!(!repo.path().join(".opencode/plugins/clumsies.ts").exists());
    assert!(
        state
            .install_project_agent_adapter(old_request)
            .await
            .is_err()
    );
    let global_config = root
        .path()
        .join("agent-host-home/.config/opencode/opencode.json");
    assert!(global_config.exists());
    state
        .remove_project_binding(DaemonProjectBindingRemoveRequest {
            workspace_root: workspace,
            expected_revision: 1,
        })
        .await
        .unwrap();
    assert!(
        global_config.exists(),
        "Removing a repository must not uninstall a harness"
    );
    request.enabled = false;
    state.set_agent_adapter(request.clone()).await.unwrap();
    state.set_agent_adapter(request).await.unwrap();
    assert!(!global_config.exists());
    for enabled in [true, true, false] {
        let settings = state
            .set_agent_adapter(daemon::DaemonSetAgentAdapterRequest {
                adapter: ProjectAgentAdapterKind::Dsh,
                enabled,
                runtime_binary_path: runtime.display().to_string(),
                host_binary_path: None,
            })
            .await
            .unwrap();
        let dsh = settings
            .items
            .iter()
            .find(|item| item.adapter == ProjectAgentAdapterKind::Dsh)
            .unwrap();
        assert_eq!(dsh.enabled, enabled);
        assert!(dsh.configured);
        assert!(
            !root
                .path()
                .join("agent-host-home/.dsh/clumsies.json")
                .exists()
        );
    }
    pool.close().await;
    drop(state);
    let reopened = common::initialize_daemon(config, common::TestCredentialStore::default()).await;
    let settings = reopened.agent_adapter_settings().await.unwrap();
    let selected = settings
        .items
        .iter()
        .find(|item| item.adapter == ProjectAgentAdapterKind::Opencode)
        .unwrap();
    assert!(!selected.enabled && selected.configured);
}

#[tokio::test]
async fn project_agent_adapter_install_is_reversible_and_repository_binding_can_then_be_removed() {
    let app = Router::new().route("/api/v1/projects/{project_id}", get(accessible_project));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let daemon_root = tempfile::tempdir().unwrap();
    let repository_root = tempfile::tempdir().unwrap();
    let agent_config = repository_root.path().join("opencode.json");
    std::fs::write(&agent_config, r#"{"model":"gpt"}"#).unwrap();
    let mut config = DaemonConfig::for_root(daemon_root.path());
    config.project.server_url = format!("http://{address}");
    let credential_store = common::TestCredentialStore::new(Some(ServerCredentials {
        server_url: config.project.server_url.clone(),
        access_token: "test-token".to_owned(),
        refresh_token: None,
    }));
    let state = common::initialize_daemon(config, credential_store).await;
    let service = DaemonIpcService::new(state);
    let binding = service
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: repository_root.path().display().to_string(),
            project_id: "prj_adapter".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();

    let helper = signed_runtime_binary(daemon_root.path());
    let installed = service
        .install_project_agent_adapter(DaemonProjectAgentAdapterInstallRequest {
            project_id: "prj_adapter".to_owned(),
            workspace_root: repository_root.path().display().to_string(),
            adapter: ProjectAgentAdapterKind::Opencode,
            runtime_binary_path: helper.display().to_string(),
            host_binary_path: None,
            expected_revision: None,
        })
        .await
        .unwrap();
    assert_eq!(installed.revision, 1);
    let rendered_config = std::fs::read_to_string(&agent_config).unwrap();
    assert!(rendered_config.contains("\"model\""));
    assert!(rendered_config.contains("\"clumsies\""));
    assert!(
        rendered_config.contains(
            &std::fs::canonicalize(&helper)
                .unwrap()
                .display()
                .to_string()
        )
    );
    assert!(!daemon_root.path().join("bin/clumsies").exists());
    // Thin skills were retired (ISSUE-064): the adapter never installs them.
    assert!(
        !repository_root
            .path()
            .join(".agents/skills/activate/SKILL.md")
            .exists()
    );

    let idempotent = service
        .install_project_agent_adapter(DaemonProjectAgentAdapterInstallRequest {
            project_id: "prj_adapter".to_owned(),
            workspace_root: repository_root.path().display().to_string(),
            adapter: ProjectAgentAdapterKind::Opencode,
            runtime_binary_path: helper.display().to_string(),
            host_binary_path: None,
            expected_revision: None,
        })
        .await
        .unwrap();
    assert_eq!(idempotent.revision, 1);
    let adapters = service
        .list_project_agent_adapters(DaemonProjectAgentAdapterListRequest {
            project_id: "prj_adapter".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(adapters.items, vec![idempotent.clone()]);

    service
        .remove_project_agent_adapter(DaemonProjectAgentAdapterRemoveRequest {
            workspace_root: repository_root.path().display().to_string(),
            adapter: ProjectAgentAdapterKind::Opencode,
            expected_revision: installed.revision,
        })
        .await
        .unwrap();
    let restored_config = std::fs::read_to_string(&agent_config).unwrap();
    assert!(restored_config.contains("\"model\""));
    assert!(!restored_config.contains("\"clumsies\""));
    assert!(
        !repository_root
            .path()
            .join(".agents/skills/activate/SKILL.md")
            .exists()
    );
    assert!(!repository_root.path().join(".agents/skills").exists());

    let removed = service
        .remove_project_binding(DaemonProjectBindingRemoveRequest {
            workspace_root: repository_root.path().display().to_string(),
            expected_revision: binding.revision,
        })
        .await
        .unwrap();
    assert!(removed.removed);
    let bindings = service
        .list_project_bindings(DaemonProjectBindingListRequest {
            project_id: "prj_adapter".to_owned(),
        })
        .await
        .unwrap();
    assert!(bindings.items.is_empty());
}

#[tokio::test]
async fn adapter_update_retires_stale_managed_skill_files() {
    let app = Router::new().route("/api/v1/projects/{project_id}", get(accessible_project));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let daemon_root = tempfile::tempdir().unwrap();
    let repository_root = tempfile::tempdir().unwrap();
    let codex_config = repository_root.path().join(".codex/config.toml");
    std::fs::create_dir_all(codex_config.parent().unwrap()).unwrap();
    std::fs::write(&codex_config, "[model]\nname = \"gpt\"\n").unwrap();
    let mut config = DaemonConfig::for_root(daemon_root.path());
    config.project.server_url = format!("http://{address}");
    let server_url = config.project.server_url.clone();
    let credential_store = common::TestCredentialStore::new(Some(ServerCredentials {
        server_url: server_url.clone(),
        access_token: "test-token".to_owned(),
        refresh_token: None,
    }));
    let state = common::initialize_daemon(config, credential_store).await;
    let service = DaemonIpcService::new(state);
    service
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: repository_root.path().display().to_string(),
            project_id: "prj_adapter".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();

    let helper = signed_runtime_binary(daemon_root.path());
    let installed = service
        .install_project_agent_adapter(DaemonProjectAgentAdapterInstallRequest {
            project_id: "prj_adapter".to_owned(),
            workspace_root: repository_root.path().display().to_string(),
            adapter: ProjectAgentAdapterKind::ClaudeCode,
            runtime_binary_path: helper.display().to_string(),
            host_binary_path: None,
            expected_revision: None,
        })
        .await
        .unwrap();
    assert_eq!(installed.revision, 1);

    // Simulate the pre-retirement state: thin skill files exist on disk and
    // the stored manifest still manages them (as older releases wrote it).
    let activate = repository_root
        .path()
        .join(".claude/skills/activate/SKILL.md");
    let ntmd = repository_root.path().join(".claude/skills/ntmd/SKILL.md");
    std::fs::create_dir_all(activate.parent().unwrap()).unwrap();
    std::fs::create_dir_all(ntmd.parent().unwrap()).unwrap();
    let content = b"---\nname: legacy\n---\n";
    std::fs::write(&activate, content).unwrap();
    std::fs::write(&ntmd, content).unwrap();
    let db = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        daemon_root.path().join("local.db").display()
    ))
    .await
    .unwrap();
    // The daemon stores the canonicalized workspace root (/private/var/...).
    let canonical_root = std::fs::canonicalize(repository_root.path()).unwrap();
    let manifest_json: String = sqlx::query_scalar(
        "SELECT manifest_json FROM project_agent_adapters
         WHERE workspace_root = $1 AND adapter = 'claude-code'",
    )
    .bind(canonical_root.display().to_string())
    .fetch_one(&db)
    .await
    .unwrap();
    let mut manifest: serde_json::Value = serde_json::from_str(&manifest_json).unwrap();
    let hash = format!("{:x}", Sha256::digest(content));
    for relative in [
        ".claude/skills/activate/SKILL.md",
        ".claude/skills/ntmd/SKILL.md",
    ] {
        manifest["managed_files"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "path": canonical_root.join(relative).display().to_string(),
                "kind": "exclusive",
                "installed_hash": hash,
            }));
    }
    sqlx::query("UPDATE project_agent_adapters SET manifest_json = $1")
        .bind(manifest.to_string())
        .execute(&db)
        .await
        .unwrap();

    // Updating the adapter must retire the previously-managed skill files.
    let updated = service
        .install_project_agent_adapter(DaemonProjectAgentAdapterInstallRequest {
            project_id: "prj_adapter".to_owned(),
            workspace_root: repository_root.path().display().to_string(),
            adapter: ProjectAgentAdapterKind::ClaudeCode,
            runtime_binary_path: helper.display().to_string(),
            host_binary_path: None,
            expected_revision: Some(installed.revision),
        })
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert!(!activate.exists());
    assert!(!ntmd.exists());
    assert!(!repository_root.path().join(".claude/skills").exists());

    let manifest_json: String = sqlx::query_scalar(
        "SELECT manifest_json FROM project_agent_adapters
         WHERE workspace_root = $1 AND adapter = 'claude-code'",
    )
    .bind(canonical_root.display().to_string())
    .fetch_one(&db)
    .await
    .unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_json).unwrap();
    assert!(
        manifest["managed_files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|file| !file["path"].as_str().unwrap().contains("skills"))
    );
}

#[tokio::test]
async fn opencode_project_agent_adapter_install_is_reversible_and_preserves_user_config() {
    let app = Router::new().route("/api/v1/projects/{project_id}", get(accessible_project));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let daemon_root = tempfile::tempdir().unwrap();
    let repository_root = tempfile::tempdir().unwrap();
    let config_path = repository_root.path().join("opencode.json");
    std::fs::write(
        &config_path,
        r#"{"$schema":"https://opencode.ai/config.json","model":"deepseek/deepseek-chat"}"#,
    )
    .unwrap();
    let mut config = DaemonConfig::for_root(daemon_root.path());
    config.project.server_url = format!("http://{address}");
    let credential_store = common::TestCredentialStore::new(Some(ServerCredentials {
        server_url: config.project.server_url.clone(),
        access_token: "test-token".to_owned(),
        refresh_token: None,
    }));
    let state = common::initialize_daemon(config, credential_store).await;
    let service = DaemonIpcService::new(state);
    let binding = service
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: repository_root.path().display().to_string(),
            project_id: "prj_opencode".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();

    let helper = signed_runtime_binary(daemon_root.path());
    let installed = service
        .install_project_agent_adapter(DaemonProjectAgentAdapterInstallRequest {
            project_id: "prj_opencode".to_owned(),
            workspace_root: repository_root.path().display().to_string(),
            adapter: ProjectAgentAdapterKind::Opencode,
            runtime_binary_path: helper.display().to_string(),
            host_binary_path: None,
            expected_revision: None,
        })
        .await
        .unwrap();
    assert_eq!(installed.revision, 1);
    assert!(
        installed
            .managed_files
            .iter()
            .any(|file| file.ends_with("opencode.json"))
    );
    assert_eq!(installed.managed_files.len(), 1);

    let rendered: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(rendered["model"], "deepseek/deepseek-chat");
    assert_eq!(rendered["$schema"], "https://opencode.ai/config.json");
    assert_eq!(rendered["mcp"]["clumsies"]["type"], "local");
    assert!(rendered.get("plugin").is_none());
    assert!(
        !repository_root
            .path()
            .join(".opencode/plugins/clumsies.ts")
            .exists()
    );

    let idempotent = service
        .install_project_agent_adapter(DaemonProjectAgentAdapterInstallRequest {
            project_id: "prj_opencode".to_owned(),
            workspace_root: repository_root.path().display().to_string(),
            adapter: ProjectAgentAdapterKind::Opencode,
            runtime_binary_path: helper.display().to_string(),
            host_binary_path: None,
            expected_revision: None,
        })
        .await
        .unwrap();
    assert_eq!(idempotent.revision, 1);
    let adapters = service
        .list_project_agent_adapters(DaemonProjectAgentAdapterListRequest {
            project_id: "prj_opencode".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(adapters.items, vec![idempotent.clone()]);

    service
        .remove_project_agent_adapter(DaemonProjectAgentAdapterRemoveRequest {
            workspace_root: repository_root.path().display().to_string(),
            adapter: ProjectAgentAdapterKind::Opencode,
            expected_revision: installed.revision,
        })
        .await
        .unwrap();
    let restored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(restored["model"], "deepseek/deepseek-chat");
    assert_eq!(restored["$schema"], "https://opencode.ai/config.json");
    assert!(restored.get("mcp").is_none());
    assert!(restored.get("plugin").is_none());
    assert!(
        !repository_root
            .path()
            .join(".opencode/plugins/clumsies.ts")
            .exists()
    );

    let removed = service
        .remove_project_binding(DaemonProjectBindingRemoveRequest {
            workspace_root: repository_root.path().display().to_string(),
            expected_revision: binding.revision,
        })
        .await
        .unwrap();
    assert!(removed.removed);
    let bindings = service
        .list_project_bindings(DaemonProjectBindingListRequest {
            project_id: "prj_opencode".to_owned(),
        })
        .await
        .unwrap();
    assert!(bindings.items.is_empty());
}

#[tokio::test]
async fn codex_host_plugin_runtime_requires_only_a_project_binding() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let daemon_root = root.path().join("daemon");
    let mut config = DaemonConfig::for_root(&daemon_root);
    config.project.server_url = "https://app.clumsies.ai".to_owned();
    let state = common::initialize_daemon(config, common::TestCredentialStore::default()).await;
    let workspace_root = std::fs::canonicalize(workspace.path())
        .unwrap()
        .display()
        .to_string();
    let db = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        daemon_root.join("local.db").display()
    ))
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO project_bindings
             (server_url, workspace_root, project_id, revision)
         VALUES ('https://app.clumsies.ai', $1, 'prj_plugin_gate', 1)",
    )
    .bind(&workspace_root)
    .execute(&db)
    .await
    .unwrap();
    let install_error = state
        .install_project_agent_adapter(DaemonProjectAgentAdapterInstallRequest {
            project_id: "prj_plugin_gate".to_owned(),
            workspace_root: workspace_root.clone(),
            adapter: ProjectAgentAdapterKind::Codex,
            runtime_binary_path: "/missing/clumsiesd".to_owned(),
            host_binary_path: None,
            expected_revision: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(install_error, DaemonError::InvalidRequest(_)));
    let request = || DaemonProjectBindingResolveRequest {
        workspace_path: workspace.path().display().to_string(),
        required_adapter: Some(ProjectAgentAdapterRuntimeRequirement {
            adapter: ProjectAgentAdapterKind::Codex,
            delivery: ProjectAgentAdapterDelivery::HostPlugin,
        }),
    };
    let accepted = state.resolve_project_binding(request()).await.unwrap();
    assert_eq!(accepted.project_id, "prj_plugin_gate");

    let other_host = state
        .resolve_project_binding(DaemonProjectBindingResolveRequest {
            workspace_path: workspace.path().display().to_string(),
            required_adapter: Some(ProjectAgentAdapterRuntimeRequirement {
                adapter: ProjectAgentAdapterKind::ClaudeCode,
                delivery: ProjectAgentAdapterDelivery::LegacyFiles,
            }),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        other_host,
        DaemonError::State {
            code: "project_agent_adapter_not_enabled",
            ..
        }
    ));
}

#[tokio::test]
async fn resolving_an_unbound_workspace_reports_project_binding_not_found() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("unbound");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut config = DaemonConfig::for_root(root.path().join("daemon"));
    config.project.server_url = "https://app.clumsies.ai".to_owned();
    let state = common::initialize_daemon(config, common::TestCredentialStore::default()).await;
    let error = state
        .resolve_project_binding(DaemonProjectBindingResolveRequest {
            workspace_path: workspace.display().to_string(),
            required_adapter: Some(ProjectAgentAdapterRuntimeRequirement {
                adapter: ProjectAgentAdapterKind::Codex,
                delivery: ProjectAgentAdapterDelivery::HostPlugin,
            }),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        DaemonError::State {
            code: "project_binding_not_found",
            ..
        }
    ));
}

#[tokio::test]
async fn commit_sync_installs_every_bound_project_independently_of_desktop_selection() {
    let probe = CommitSyncProbeState {
        org_requests: Arc::new(AtomicUsize::new(0)),
        project_requests: Arc::new(AtomicUsize::new(0)),
        project_in_flight: Arc::new(AtomicUsize::new(0)),
        max_project_in_flight: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/projects/{project_id}", get(accessible_project))
        .route(
            "/api/v1/projects/{project_id}/commit-state",
            get(probed_empty_project_commit_state),
        )
        .route("/api/v1/me", get(bindings_identity))
        .route(
            "/api/v1/org/commit-state",
            get(probed_empty_org_commit_state),
        )
        .with_state(probe.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let daemon_root = tempfile::tempdir().unwrap();
    let workspaces = tempfile::tempdir().unwrap();
    let first_root = workspaces.path().join("first");
    let second_root = workspaces.path().join("second");
    std::fs::create_dir_all(&first_root).unwrap();
    std::fs::create_dir_all(&second_root).unwrap();
    let mut config = DaemonConfig::for_root(daemon_root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_desktop_selection".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;

    state
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: first_root.display().to_string(),
            project_id: "prj_bound_first".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();
    state
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: second_root.display().to_string(),
            project_id: "prj_bound_second".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    for project_id in ["prj_bound_first", "prj_bound_second"] {
        sqlx::query("INSERT INTO daemon_meta (key, value) VALUES ($1, 'previous scoped failure')")
            .bind(format!("commit_sync_last_error:{project_id}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    pool.close().await;
    state
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap();

    for project_id in ["prj_bound_first", "prj_bound_second"] {
        let cache = state
            .memory_cache(DaemonMemoryCacheRequest {
                project_id: project_id.to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(cache.state, DaemonMemoryCacheState::Ready);
        assert!(
            state
                .project_sync_status(DaemonProjectSyncStatusRequest {
                    project_id: project_id.to_owned(),
                })
                .await
                .unwrap()
                .commit_sync
                .last_error
                .is_none()
        );
    }
    assert_eq!(probe.org_requests.load(Ordering::Acquire), 1);
    assert_eq!(probe.project_requests.load(Ordering::Acquire), 3);
    let max_project_in_flight = probe.max_project_in_flight.load(Ordering::Acquire);
    assert!(max_project_in_flight > 1);
    assert!(max_project_in_flight <= 4);
}

#[tokio::test]
async fn global_commit_sync_records_failure_and_success_per_project() {
    let app = Router::new()
        .route("/api/v1/projects/{project_id}", get(accessible_project))
        .route(
            "/api/v1/projects/{project_id}/commit-state",
            get(failing_a_empty_b_project_commit_state),
        )
        .route("/api/v1/me", get(bindings_identity))
        .route("/api/v1/org/commit-state", get(empty_org_commit_state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let daemon_root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(daemon_root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_a".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    state
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: workspace.path().display().to_string(),
            project_id: "prj_b".to_owned(),
            expected_revision: None,
        })
        .await
        .unwrap();

    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO daemon_meta (key, value)
         VALUES ('commit_sync_last_error:prj_b', 'stale project B failure')",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let error = state
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("injected project A failure"),
        "unexpected sync error: {error}"
    );

    let project_a = state
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap()
        .commit_sync;
    let project_b = state
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_b".to_owned(),
        })
        .await
        .unwrap()
        .commit_sync;
    assert_eq!(project_a.state, SyncState::Failed);
    assert!(
        project_a
            .last_error
            .as_ref()
            .unwrap()
            .message
            .contains("injected project A failure")
    );
    assert!(project_a.last_attempt_at.is_some());
    assert_eq!(project_b.state, SyncState::Idle);
    assert!(project_b.last_error.is_none());
    assert!(project_b.last_attempt_at.is_some());
    assert!(project_b.last_success_at.is_some());
    assert!(project_b.last_attempt_at <= project_b.last_success_at);
    assert_eq!(
        state.sync_status().await.unwrap().commit_sync.state,
        SyncState::Failed
    );
}

#[tokio::test]
async fn project_commit_sync_status_and_retry_are_isolated() {
    let probe = ScopedCommitSyncProbeState {
        project_requests: Arc::new(Mutex::new(Vec::new())),
        started: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    };
    let app = Router::new()
        .route(
            "/api/v1/projects/{project_id}/commit-state",
            get(blocking_scoped_project_commit_state),
        )
        .route("/api/v1/me", get(bindings_identity))
        .route("/api/v1/org/commit-state", get(empty_org_commit_state))
        .with_state(probe.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_desktop_selection".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let service = DaemonIpcService::new(state.clone());
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    for (project_id, commit_id) in [("prj_a", "commit-a"), ("prj_b", "commit-b")] {
        sqlx::query(
            "INSERT INTO cached_refs (
                ref_key, name, scope, org_id, project_id, commit_id, etag,
                server_updated_at, installed_at
             ) VALUES ($1, 'refs/heads/main', 'project', 'org_bindings', $2, $3, $3,
                       '2026-08-27T00:00:00Z', '2026-08-27T00:00:00Z')",
        )
        .bind(format!("project:{project_id}"))
        .bind(project_id)
        .bind(commit_id)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO daemon_meta (key, value)
         VALUES ('commit_sync_last_error', 'failure from another Project')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let project_a = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    let project_b = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_b".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        project_a.commit_sync.server_cursor.as_deref(),
        Some("commit-a")
    );
    assert_eq!(
        project_b.commit_sync.server_cursor.as_deref(),
        Some("commit-b")
    );
    assert!(project_a.commit_sync.last_error.is_none());
    assert!(project_b.commit_sync.last_error.is_none());

    let retry_service = service.clone();
    let retry = tokio::spawn(async move {
        retry_service
            .project_retry_sync(DaemonProjectSyncRetryRequest {
                project_id: "prj_a".to_owned(),
                channel: SyncRetryChannel::Commits,
            })
            .await
    });
    probe.started.notified().await;

    let project_a = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    let project_b = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_b".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(project_a.commit_sync.state, SyncState::Syncing);
    assert_eq!(project_b.commit_sync.state, SyncState::Idle);

    probe.release.notify_one();
    retry.await.unwrap().unwrap();

    let project_a = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_a".to_owned(),
        })
        .await
        .unwrap();
    let project_b = service
        .project_sync_status(DaemonProjectSyncStatusRequest {
            project_id: "prj_b".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(project_a.commit_sync.server_cursor, None);
    assert!(project_a.commit_sync.last_attempt_at.is_some());
    assert!(project_a.commit_sync.last_success_at.is_some());
    assert!(project_a.commit_sync.last_error.is_none());
    assert_eq!(
        project_b.commit_sync.server_cursor.as_deref(),
        Some("commit-b")
    );
    assert_eq!(project_b.commit_sync.last_attempt_at, None);
    assert!(project_b.commit_sync.last_error.is_none());
    assert_eq!(
        probe.project_requests.lock().unwrap().as_slice(),
        &["prj_a".to_owned()]
    );
    assert!(
        service
            .sync_status()
            .await
            .unwrap()
            .commit_sync
            .last_error
            .is_some()
    );
}

#[tokio::test]
async fn keychain_credentials_are_bound_to_the_configured_server() {
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = "https://current.example.com".to_owned();
    config.project.project_id = Some("prj_current".to_owned());
    let credential_store = common::TestCredentialStore::new(Some(ServerCredentials {
        server_url: "https://previous.example.com".to_owned(),
        access_token: "previous-access".to_owned(),
        refresh_token: Some("previous-refresh".to_owned()),
    }));

    let state = common::initialize_daemon(config, credential_store.clone()).await;
    let project_config = state.project_config_status();

    assert_eq!(project_config.server_url, "https://current.example.com");
    assert!(!project_config.has_access_token);
    assert!(!project_config.has_refresh_token);
    assert!(!project_config.ready);
    assert!(credential_store.credentials().is_some());
}

#[tokio::test]
async fn refresh_token_without_access_token_is_rejected_before_persistence() {
    let root = tempfile::tempdir().unwrap();
    let credential_store = common::TestCredentialStore::default();
    let state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        credential_store.clone(),
    )
    .await;

    let error = state
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: "https://clumsies.example.com".to_owned(),
            project_id: Some("prj_test".to_owned()),
            memory_guidelines_path: None,
            access_token: None,
            refresh_token: Some("orphan-refresh".to_owned()),
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("without access_token"));
    assert!(credential_store.credentials().is_none());
    assert_eq!(state.project_config_status().server_url, "");
}

#[tokio::test]
async fn credential_write_failure_does_not_change_project_metadata_or_runtime_state() {
    let root = tempfile::tempdir().unwrap();
    let credential_store = common::TestCredentialStore::default();
    let state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        credential_store.clone(),
    )
    .await;
    credential_store.set_fail_writes(true);

    let error = state
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: "https://clumsies.example.com".to_owned(),
            project_id: Some("prj_test".to_owned()),
            memory_guidelines_path: None,
            access_token: Some("access-secret".to_owned()),
            refresh_token: Some("refresh-secret".to_owned()),
        })
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("injected credential write failure")
    );
    assert!(credential_store.credentials().is_none());
    let project_config = state.project_config_status();
    assert_eq!(project_config.server_url, "");
    assert_eq!(project_config.project_id, None);
    assert!(!project_config.has_access_token);

    let metadata_pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        root.path().join("local.db").display()
    ))
    .await
    .unwrap();
    let persisted_config_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM daemon_meta
         WHERE key IN ('project_config_server_url', 'project_config_project_id')",
    )
    .fetch_one(&metadata_pool)
    .await
    .unwrap();
    assert_eq!(persisted_config_count, 0);
    metadata_pool.close().await;
}

#[tokio::test]
async fn sync_retry_uploads_new_local_draft_to_server() {
    let server = FakeServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = server.url.clone();
    config.project.project_id = Some("prj_default".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO cached_commits (
            commit_id, scope, org_id, project_id, tree_id,
            parent_commit_id, version, created_at, payload_json
         ) VALUES ($1, 'project', 'org_test', 'prj_test', 'tree_test',
                   NULL, 1, '2026-07-08T00:00:00Z', '{}')",
    )
    .bind(COMMIT_A)
    .execute(&pool)
    .await
    .unwrap();
    let service = DaemonIpcService::new(state.clone());

    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: Some(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            ),
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "docs/sync.md".to_owned(),
                    content: context_content("Sync me"),
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

    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    let status = service.sync_status().await.unwrap();
    assert_eq!(status.pending_operation_count, 0);
    assert_eq!(status.failed_operation_count, 0);
    assert_eq!(status.draft_sync.state, SyncState::Idle);
    assert_eq!(status.draft_sync.server_cursor.as_deref(), Some("42"));
    assert!(status.draft_sync.last_attempt_at.is_some());
    assert!(status.draft_sync.last_success_at.is_some());
    assert_eq!(status.last_success_at, status.draft_sync.last_success_at);

    let drafts = service
        .list_drafts(DaemonDraftListQuery::default())
        .await
        .unwrap();
    assert_eq!(drafts.items.len(), 1);
    assert_eq!(drafts.items[0].server_version, 2);
    let projected = service.get_draft(&drafts.items[0].draft_id).await.unwrap();
    assert_eq!(projected.operations.len(), 2);
    assert_eq!(
        projected.operations[0].source,
        DaemonDraftOperationRecordSource::Desktop
    );
    assert_eq!(
        projected.operations[1].source,
        DaemonDraftOperationRecordSource::Server
    );
    assert_eq!(
        projected.operations[1]
            .operation
            .create
            .as_ref()
            .unwrap()
            .content,
        context_content("Remote revision")
    );

    let pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    let remote_event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remote_draft_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    let cursor: String =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'draft_events_cursor'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remote_event_count, 1);
    assert_eq!(cursor, "42");

    let requests = server.create_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].get("author_user_id").is_none());
    assert_eq!(requests[0]["project_id"], "prj_test");
    assert_eq!(requests[0]["resource"]["scope"], "org");
    assert_eq!(
        requests[0]["base_commit_id"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert!(
        requests[0]["daemon_installation_id"]
            .as_str()
            .unwrap()
            .starts_with("daemon_")
    );
    assert!(requests[0]["resource"].get("kind").is_none());
    assert_eq!(requests[0]["resource"]["path"], "docs/sync.md");
    assert_eq!(requests[0]["operations"][0]["action"], "create");
    assert_eq!(
        requests[0]["operations"][0]["content"]["content"],
        "Sync me"
    );
}

#[tokio::test]
async fn concurrent_explicit_sync_retries_wait_for_the_inflight_sync() {
    let blocking_state = BlockingDraftEventsState {
        started: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
        request_count: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/me", get(fake_identity))
        .route("/api/v1/draft-events", get(blocking_draft_events))
        .with_state(blocking_state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_retry_lock".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let service = DaemonIpcService::new(state);

    let first_service = service.clone();
    let first = tokio::spawn(async move {
        first_service
            .retry_sync(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::Drafts,
            })
            .await
            .unwrap();
    });
    tokio::time::timeout(Duration::from_secs(5), blocking_state.started.notified())
        .await
        .unwrap();

    let second_service = service.clone();
    let second = tokio::spawn(async move {
        second_service
            .retry_sync(DaemonSyncRetryRequest {
                channel: SyncRetryChannel::Drafts,
            })
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!second.is_finished());
    assert_eq!(blocking_state.request_count.load(Ordering::Acquire), 1);

    blocking_state.release.notify_one();
    first.await.unwrap();
    second.await.unwrap();
    assert_eq!(blocking_state.request_count.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn terminal_draft_projection_fetches_are_bounded_and_skip_base_commits() {
    let projection_state = TerminalDraftProjectionState {
        request_count: Arc::new(AtomicUsize::new(0)),
        in_flight: Arc::new(AtomicUsize::new(0)),
        max_in_flight: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/me", get(fake_identity))
        .route("/api/v1/draft-events", get(terminal_draft_events))
        .route("/api/v1/drafts/{draft_id}", get(terminal_draft_projection))
        .with_state(projection_state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_terminal".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let service = DaemonIpcService::new(state);

    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    assert_eq!(projection_state.request_count.load(Ordering::Acquire), 12);
    let max_in_flight = projection_state.max_in_flight.load(Ordering::Acquire);
    assert!(max_in_flight > 1);
    assert!(max_in_flight <= 8);
    assert_eq!(
        service
            .sync_status()
            .await
            .unwrap()
            .draft_sync
            .server_cursor,
        Some("terminal:12".to_owned())
    );
}

#[tokio::test]
async fn failed_remote_projection_does_not_advance_the_event_cursor() {
    let app = Router::new()
        .route("/api/v1/me", get(fake_identity))
        .route("/api/v1/draft-events", get(fake_list_draft_events))
        .route("/api/v1/drafts/{draft_id}", get(fake_stale_draft));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_test".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let service = DaemonIpcService::new(state.clone());

    let error = service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("inconsistent projection"));
    assert_eq!(
        service
            .sync_status()
            .await
            .unwrap()
            .draft_sync
            .server_cursor,
        None
    );
    assert!(
        service
            .list_drafts(DaemonDraftListQuery::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remote_draft_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_count, 0);
}

#[tokio::test]
async fn server_reconciliation_projection_keeps_lifecycle_separate_from_coordination() {
    let conflict_state = FakeConflictProjectionState {
        resolved: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/me", get(fake_identity))
        .route("/api/v1/draft-events", get(fake_conflicted_draft_events))
        .route("/api/v1/drafts/{draft_id}", get(fake_conflicted_draft))
        .with_state(conflict_state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_conflict".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    for (commit_id, tree_id) in [(COMMIT_A, "tree_a"), (COMMIT_B, "tree_b")] {
        sqlx::query(
            "INSERT INTO cached_commits (
                commit_id, scope, org_id, project_id, tree_id,
                parent_commit_id, version, created_at, payload_json
             ) VALUES ($1, 'project', 'org_test', 'prj_conflict', $2,
                       NULL, 1, '2026-07-15T00:00:00Z', '{}')",
        )
        .bind(commit_id)
        .bind(tree_id)
        .execute(&pool)
        .await
        .unwrap();
    }
    let service = DaemonIpcService::new(state);

    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    let drafts = service
        .list_drafts(DaemonDraftListQuery::default())
        .await
        .unwrap();
    assert_eq!(drafts.items.len(), 1);
    let draft = &drafts.items[0];
    assert_eq!(draft.status, DaemonLocalDraftStatus::Submitted);
    assert_eq!(draft.base_commit_id.as_deref(), Some(COMMIT_A));
    assert_eq!(draft.current_commit_id.as_deref(), Some(COMMIT_B));
    assert_eq!(draft.freshness, daemon::DaemonDraftFreshness::Behind);
    assert!(draft.has_upstream_resource_changes);
    assert_eq!(
        draft.reconciliation,
        daemon::DaemonDraftReconciliationStatus::Conflicts
    );
    assert_eq!(
        draft.reconciliation_candidate_id.as_deref(),
        Some("rcn_conflict")
    );

    let sync = service.sync_status().await.unwrap();
    assert_eq!(sync.behind_draft_count, 1);
    assert_eq!(sync.reconciliation_conflict_count, 1);
    assert_eq!(sync.draft_sync.state, SyncState::Idle);
    assert_eq!(sync.failed_operation_count, 0);

    conflict_state.resolved.store(1, Ordering::SeqCst);
    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    let resolved = service.get_draft("drf_conflict").await.unwrap();
    assert_eq!(resolved.draft.status, DaemonLocalDraftStatus::Submitted);
    assert_eq!(resolved.draft.base_commit_id.as_deref(), Some(COMMIT_B));
    assert_eq!(resolved.draft.current_commit_id.as_deref(), Some(COMMIT_B));
    assert_eq!(
        resolved.draft.freshness,
        daemon::DaemonDraftFreshness::Current
    );
    assert_eq!(
        resolved.draft.reconciliation,
        daemon::DaemonDraftReconciliationStatus::Unknown
    );
    assert_eq!(resolved.draft.reconciliation_candidate_id, None);
    assert_eq!(resolved.operations.len(), 1);
    assert_eq!(
        resolved.operations[0]
            .operation
            .update
            .as_ref()
            .unwrap()
            .content(),
        Some(&context_content("Resolved content"))
    );
    let sync = service.sync_status().await.unwrap();
    assert_eq!(sync.behind_draft_count, 0);
    assert_eq!(sync.reconciliation_conflict_count, 0);
    assert_eq!(sync.draft_sync.state, SyncState::Idle);
}

#[tokio::test]
async fn queued_draft_syncs_after_project_config_is_set() {
    let server = FakeServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.sync.enabled = true;
    config.sync.interval = Duration::from_secs(60);
    let state = common::initialize_daemon(config, common::TestCredentialStore::default()).await;
    let worker = state.start_sync_worker().unwrap();
    let service = DaemonIpcService::new(state);

    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_late".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "docs/later-config.md".to_owned(),
                    content: context_content("sync after config"),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: None,
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let before = service.sync_status().await.unwrap();
    assert_eq!(before.pending_operation_count, 1);
    assert_eq!(before.failed_operation_count, 0);
    assert_eq!(before.draft_sync.state, SyncState::Degraded);
    assert_eq!(before.draft_sync.server_cursor, None);
    assert_eq!(before.draft_sync.last_attempt_at, None);
    assert_eq!(before.draft_sync.last_success_at, None);
    assert_eq!(
        before.draft_sync.last_error.unwrap().code,
        "daemon_project_config_incomplete"
    );
    assert!(server.create_requests.lock().unwrap().is_empty());

    service
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: server.url.clone(),
            project_id: Some("prj_late".to_owned()),
            memory_guidelines_path: None,
            access_token: Some("test-token".to_owned()),
            refresh_token: None,
        })
        .await
        .unwrap();

    wait_for_create_request(&server).await;
    let after = service.sync_status().await.unwrap();
    assert_eq!(after.pending_operation_count, 0);
    assert_eq!(after.failed_operation_count, 0);
    assert_eq!(after.draft_sync.state, SyncState::Idle);

    let requests = server.create_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].get("author_user_id").is_none());
    assert_eq!(requests[0]["project_id"], "prj_late");
    assert_eq!(
        requests[0]["operations"][0]["content"]["content"],
        "sync after config"
    );
    drop(requests);
    worker.abort();
}

#[tokio::test]
async fn draft_operation_notifies_auto_sync_worker() {
    let server = FakeServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = server.url.clone();
    config.project.project_id = Some("prj_test".to_owned());
    config.sync.enabled = true;
    config.sync.interval = Duration::from_secs(60);
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let worker = state.start_sync_worker().unwrap();
    let service = DaemonIpcService::new(state);

    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "docs/auto.md".to_owned(),
                    content: context_content("automatic"),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: None,
        })
        .await
        .unwrap();

    wait_for_create_request(&server).await;
    let status = wait_for_draft_sync_idle(&service).await;
    assert_eq!(status.pending_operation_count, 0);
    assert_eq!(status.failed_operation_count, 0);

    let requests = server.create_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["operations"][0]["content"]["content"],
        "automatic"
    );
    drop(requests);
    worker.abort();
}

#[tokio::test]
async fn transient_server_failure_is_retried_automatically() {
    let server_state = RecoveringServerState {
        available: Arc::new(AtomicBool::new(false)),
        create_request_count: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/drafts", post(recovering_create_draft))
        .route("/api/v1/draft-events", get(empty_draft_events))
        .route("/api/v1/me", get(fake_identity))
        .route("/api/v1/org/commit-state", get(fake_org_commit_state))
        .route(
            "/api/v1/projects/{project_id}/commit-state",
            get(fake_project_commit_state),
        )
        .with_state(server_state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_recovery".to_owned());
    config.sync.enabled = true;
    config.sync.interval = Duration::from_millis(50);
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let worker = state.start_sync_worker().unwrap();
    let service = DaemonIpcService::new(state);

    let stored = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_recovery".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/offline.md".to_owned(),
                    content: context_content("Saved while Server is unavailable"),
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

    let retrying = wait_for_operation_status(
        &service,
        &stored.draft_id,
        DraftOperationSyncStatus::Retrying,
    )
    .await;
    assert_eq!(retrying.draft.pending_operation_count, 1);
    let status = service.sync_status().await.unwrap();
    assert_eq!(status.draft_sync.state, SyncState::Retrying);
    assert_eq!(status.pending_operation_count, 1);
    assert_eq!(status.failed_operation_count, 0);
    assert!(status.draft_sync.last_attempt_at.is_some());
    assert!(status.draft_sync.last_error.is_some());

    server_state.available.store(true, Ordering::Release);
    let synced =
        wait_for_operation_status(&service, &stored.draft_id, DraftOperationSyncStatus::Synced)
            .await;
    assert_eq!(synced.draft.pending_operation_count, 0);
    assert_eq!(synced.draft.failed_operation_count, 0);
    assert!(server_state.create_request_count.load(Ordering::Acquire) >= 2);
    assert_eq!(
        service.sync_status().await.unwrap().draft_sync.state,
        SyncState::Idle
    );
    worker.abort();
}

#[tokio::test]
async fn successful_new_draft_does_not_hide_an_existing_retrying_operation() {
    let server_state = RecoveringServerState {
        available: Arc::new(AtomicBool::new(true)),
        create_request_count: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/drafts", post(recovering_create_draft))
        .route("/api/v1/draft-events", get(empty_draft_events))
        .route("/api/v1/me", get(fake_identity))
        .route("/api/v1/org/commit-state", get(fake_org_commit_state))
        .route(
            "/api/v1/projects/{project_id}/commit-state",
            get(fake_project_commit_state),
        )
        .with_state(server_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_retrying".to_owned());
    config.sync.enabled = true;
    config.sync.interval = Duration::from_secs(60);
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let worker = state.start_sync_worker().unwrap();
    let service = DaemonIpcService::new(state.clone());

    let mut baseline_last_success = None;
    for _ in 0..100 {
        if let Some(last_success) = service
            .sync_status()
            .await
            .unwrap()
            .draft_sync
            .last_success_at
        {
            baseline_last_success = Some(last_success);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let baseline_last_success = baseline_last_success.expect("initial sync cycle did not complete");

    let pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO local_drafts (
            draft_id, project_id, resource_scope, resource_kind, status
         ) VALUES ('draft_retrying', 'prj_retrying', 'project', 'memory', 'open')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO local_draft_operations (
            local_operation_id, draft_id, resource_kind, operation_json, source, sync_status,
            last_error
         ) VALUES (
            'operation_retrying', 'draft_retrying', 'memory', '{}', 'desktop', 'retrying',
            'Server unavailable'
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let stored = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_retrying".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/synced-while-retrying.md".to_owned(),
                    content: context_content("This operation can sync"),
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
    wait_for_operation_status(&service, &stored.draft_id, DraftOperationSyncStatus::Synced).await;

    let status = service.sync_status().await.unwrap();
    assert_eq!(status.draft_sync.state, SyncState::Retrying);
    assert_eq!(status.pending_operation_count, 1);
    assert_eq!(
        status.draft_sync.last_success_at.as_deref(),
        Some(baseline_last_success.as_str())
    );
    worker.abort();
}

#[tokio::test]
async fn daemon_restart_requeues_an_interrupted_operation() {
    let root = tempfile::tempdir().unwrap();
    let credential_store = common::TestCredentialStore::default();
    let state = common::initialize_daemon(
        DaemonConfig::for_root(root.path()),
        credential_store.clone(),
    )
    .await;
    let service = DaemonIpcService::new(state.clone());
    let stored = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_restart".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "context/restart.md".to_owned(),
                    content: context_content("Recover after process exit"),
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
    let pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE local_draft_operations SET sync_status = 'syncing' WHERE local_operation_id = $1",
    )
    .bind(&stored.local_operation_id)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    drop(service);
    drop(state);

    let restarted =
        common::initialize_daemon(DaemonConfig::for_root(root.path()), credential_store).await;
    let detail = DaemonIpcService::new(restarted)
        .get_draft(&stored.draft_id)
        .await
        .unwrap();
    assert_eq!(
        detail.operations[0].sync_status,
        DraftOperationSyncStatus::Queued
    );
    assert_eq!(detail.draft.pending_operation_count, 1);
    assert_eq!(detail.draft.failed_operation_count, 0);
}

#[tokio::test]
async fn server_proxy_preserves_contract_headers_and_rejects_caller_credentials() {
    let app = Router::new().route("/api/v1/header-probe", post(proxy_header_probe));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_headers".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;

    let response = state
        .server_request(DaemonServerRequest {
            method: "POST".to_owned(),
            path: "/api/v1/header-probe".to_owned(),
            headers: BTreeMap::from([
                ("IdEmPoTeNcY-Key".to_owned(), "project-create-1".to_owned()),
                ("If-Match".to_owned(), "17".to_owned()),
                ("IF-NONE-MATCH".to_owned(), "\"etag\"".to_owned()),
                ("Authorization".to_owned(), "Bearer caller-token".to_owned()),
                ("Cookie".to_owned(), "session=caller".to_owned()),
                ("X-Untrusted".to_owned(), "value".to_owned()),
            ]),
            body: Some("{}".to_owned()),
        })
        .await
        .unwrap();

    assert_eq!(response.status, 200);
    let probe: ProxyHeaderProbe = serde_json::from_str(&response.body).unwrap();
    assert_eq!(probe.idempotency_key.as_deref(), Some("project-create-1"));
    assert_eq!(probe.if_match.as_deref(), Some("17"));
    assert_eq!(probe.if_none_match.as_deref(), Some("\"etag\""));
    assert_eq!(probe.authorization, vec!["Bearer test-token"]);
    assert_eq!(probe.cookie, None);
    assert_eq!(probe.untrusted, None);

    server.abort();
}
#[tokio::test]
async fn concurrent_unauthorized_requests_share_a_same_session_refresh() {
    let probe = RefreshProbeState::default();
    let app = Router::new()
        .route("/api/v1/refresh-probe", post(refresh_probe_request))
        .route("/api/v1/auth/token", post(refresh_probe_token))
        .with_state(probe.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_refresh".to_owned());
    let (state, credential_store) = common::initialize_authenticated_daemon(
        config,
        "stale-token",
        Some("initial-refresh-token".to_owned()),
    )
    .await;

    let first_state = state.clone();
    let first = tokio::spawn(async move {
        first_state
            .server_request(refresh_probe_server_request())
            .await
    });
    let second_state = state.clone();
    let second = tokio::spawn(async move {
        second_state
            .server_request(refresh_probe_server_request())
            .await
    });

    tokio::time::timeout(Duration::from_secs(2), probe.refresh_started.notified())
        .await
        .expect("the first unauthorized request should start token refresh");
    tokio::time::timeout(Duration::from_secs(2), probe.both_stale.notified())
        .await
        .expect("both requests should receive 401 before refresh completes");
    probe.release_refresh.notify_one();

    let (first, second) = tokio::time::timeout(Duration::from_secs(2), async {
        (
            first.await.unwrap().unwrap(),
            second.await.unwrap().unwrap(),
        )
    })
    .await
    .expect("both requests should reuse the completed same-session refresh");
    assert_eq!(first.status, 200);
    assert_eq!(second.status, 200);
    assert_eq!(probe.stale_requests.load(Ordering::Acquire), 2);
    assert_eq!(probe.fresh_requests.load(Ordering::Acquire), 2);
    assert_eq!(probe.refresh_requests.load(Ordering::Acquire), 1);
    let credentials = credential_store.credentials().unwrap();
    assert_eq!(credentials.access_token, "fresh-token");
    assert_eq!(
        credentials.refresh_token.as_deref(),
        Some("rotated-refresh-token")
    );

    server.abort();
}

#[tokio::test]
async fn request_is_not_replayed_after_same_server_session_replacement_during_refresh() {
    let old_probe = RefreshProbeState::default();
    let old_app = Router::new()
        .route("/api/v1/refresh-probe", post(refresh_probe_request))
        .route("/api/v1/auth/token", post(refresh_probe_token))
        .with_state(old_probe.clone());
    let old_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let old_address = old_listener.local_addr().unwrap();
    let old_server = tokio::spawn(async move {
        axum::serve(old_listener, old_app).await.unwrap();
    });

    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{old_address}");
    config.project.project_id = Some("prj_old".to_owned());
    let (state, credential_store) = common::initialize_authenticated_daemon(
        config,
        "stale-token",
        Some("initial-refresh-token".to_owned()),
    )
    .await;

    let request_state = state.clone();
    let request = tokio::spawn(async move {
        request_state
            .server_request(refresh_probe_server_request())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), old_probe.refresh_started.notified())
        .await
        .expect("the old session should enter token refresh");

    state
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: format!("http://{old_address}"),
            project_id: Some("prj_replacement".to_owned()),
            memory_guidelines_path: None,
            access_token: Some("replacement-token".to_owned()),
            refresh_token: Some("replacement-refresh-token".to_owned()),
        })
        .await
        .unwrap();
    old_probe.release_refresh.notify_one();

    let response = tokio::time::timeout(Duration::from_secs(2), request)
        .await
        .expect("the superseded request should return its original 401")
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 401);
    assert_eq!(old_probe.replacement_requests.load(Ordering::Acquire), 0);
    assert_eq!(old_probe.refresh_requests.load(Ordering::Acquire), 1);
    let credentials = credential_store.credentials().unwrap();
    assert_eq!(credentials.server_url, format!("http://{old_address}"));
    assert_eq!(credentials.access_token, "replacement-token");
    assert_eq!(
        credentials.refresh_token.as_deref(),
        Some("replacement-refresh-token")
    );

    old_server.abort();
}

#[tokio::test]
async fn server_proxy_returns_live_get_while_cache_write_is_locked() {
    let proxy_state = CachedProxyState {
        unavailable: Arc::new(AtomicBool::new(false)),
    };
    let app = Router::new()
        .route("/api/v1/cache-probe", get(cached_proxy_get))
        .with_state(proxy_state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_cache_lock".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let request = DaemonServerRequest {
        method: "GET".to_owned(),
        path: "/api/v1/cache-probe".to_owned(),
        headers: BTreeMap::new(),
        body: None,
    };
    let cache_pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    let mut cache_lock = cache_pool.acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *cache_lock)
        .await
        .unwrap();

    let live = tokio::time::timeout(
        Duration::from_secs(1),
        state.server_request(request.clone()),
    )
    .await
    .expect("live GET must not wait for the SQLite cache writer")
    .unwrap();
    assert_eq!(live.status, 200);
    assert_eq!(live.body, r#"{"source":"live"}"#);

    sqlx::query("ROLLBACK")
        .execute(&mut *cache_lock)
        .await
        .unwrap();
    drop(cache_lock);
    cache_pool.close().await;
    wait_for_server_response_cache(&state, &request.path, &live.body).await;

    proxy_state.unavailable.store(true, Ordering::Release);
    let stale = state.server_request(request).await.unwrap();
    assert_eq!(stale.body, live.body);
    assert_eq!(
        stale.headers.get("x-clumsies-cache").map(String::as_str),
        Some("stale")
    );

    server.abort();
}

#[tokio::test]
async fn server_proxy_uses_stale_cache_only_for_read_failures() {
    let proxy_state = CachedProxyState {
        unavailable: Arc::new(AtomicBool::new(false)),
    };
    let app = Router::new()
        .route(
            "/api/v1/cache-probe",
            get(cached_proxy_get).post(cached_proxy_post),
        )
        .route("/api/v1/missing-probe", get(cached_missing_get))
        .with_state(proxy_state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.project.project_id = Some("prj_cache".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let get_request = DaemonServerRequest {
        method: "GET".to_owned(),
        path: "/api/v1/cache-probe".to_owned(),
        headers: BTreeMap::new(),
        body: None,
    };

    let live = state.server_request(get_request.clone()).await.unwrap();
    assert_eq!(live.status, 200);
    assert_eq!(live.body, r#"{"source":"live"}"#);
    assert_eq!(live.headers.get("x-clumsies-cache"), None);
    wait_for_server_response_cache(&state, &get_request.path, &live.body).await;

    let missing_request = DaemonServerRequest {
        method: "GET".to_owned(),
        path: "/api/v1/missing-probe".to_owned(),
        headers: BTreeMap::new(),
        body: None,
    };
    let missing = state.server_request(missing_request.clone()).await.unwrap();
    assert_eq!(missing.status, 404);
    assert_eq!(missing.headers.get("x-clumsies-cache"), None);
    wait_for_server_response_cache(&state, &missing_request.path, &missing.body).await;

    proxy_state.unavailable.store(true, Ordering::Release);
    let cached_after_503 = state.server_request(get_request.clone()).await.unwrap();
    assert_eq!(cached_after_503.status, 200);
    assert_eq!(cached_after_503.body, live.body);
    assert_eq!(
        cached_after_503
            .headers
            .get("x-clumsies-cache")
            .map(String::as_str),
        Some("stale")
    );
    let cached_missing = state.server_request(missing_request).await.unwrap();
    assert_eq!(cached_missing.status, 404);
    assert_eq!(
        cached_missing
            .headers
            .get("x-clumsies-cache")
            .map(String::as_str),
        Some("stale")
    );

    let write = state
        .server_request(DaemonServerRequest {
            method: "POST".to_owned(),
            path: "/api/v1/cache-probe".to_owned(),
            headers: BTreeMap::new(),
            body: Some("{}".to_owned()),
        })
        .await
        .unwrap();
    assert_eq!(write.status, 503);
    assert_eq!(write.headers.get("x-clumsies-cache"), None);

    server.abort();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let cached_after_disconnect = state.server_request(get_request.clone()).await.unwrap();
    assert_eq!(cached_after_disconnect.status, 200);
    assert_eq!(
        cached_after_disconnect
            .headers
            .get("x-clumsies-cache")
            .map(String::as_str),
        Some("stale")
    );

    state
        .replace_project_config(DaemonProjectConfigUpdateRequest {
            server_url: format!("http://{address}"),
            project_id: Some("prj_cache".to_owned()),
            memory_guidelines_path: None,
            access_token: Some("replacement-token".to_owned()),
            refresh_token: None,
        })
        .await
        .unwrap();
    assert!(state.server_request(get_request).await.is_err());
}

#[tokio::test]
async fn sync_retry_uploads_later_new_resource_edits_to_the_same_draft() {
    let server = FakeServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = server.url.clone();
    config.project.project_id = Some("prj_test".to_owned());
    let (state, _) = common::initialize_authenticated_daemon(config, "test-token", None).await;
    let service = DaemonIpcService::new(state);

    let first = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "docs/batch.md".to_owned(),
                    content: context_content("one"),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: None,
        })
        .await
        .unwrap();

    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    assert_eq!(
        service
            .sync_status()
            .await
            .unwrap()
            .draft_sync
            .server_cursor
            .as_deref(),
        Some("42")
    );

    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(first.draft_id.clone()),
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "docs/batch.md".to_owned(),
                    content: context_content("two"),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: None,
                discard: None,
            },
            source: None,
        })
        .await
        .unwrap();

    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();
    assert_eq!(
        service
            .sync_status()
            .await
            .unwrap()
            .draft_sync
            .server_cursor
            .as_deref(),
        Some("42")
    );

    service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: Some(first.draft_id),
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: None,
                update: None,
                rename: None,
                delete: None,
                discard: Some(DaemonDiscardDraftOperation {
                    id: "draft_local".to_owned(),
                }),
            },
            source: None,
        })
        .await
        .unwrap();

    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Drafts,
        })
        .await
        .unwrap();

    let status = service.sync_status().await.unwrap();
    assert_eq!(status.pending_operation_count, 0);
    assert_eq!(status.failed_operation_count, 0);

    let creates = server.create_requests.lock().unwrap();
    assert_eq!(creates.len(), 1);
    drop(creates);

    let batches = server.batch_requests.lock().unwrap();
    assert_eq!(batches.len(), 1);
    assert!(
        batches[0]["daemon_installation_id"]
            .as_str()
            .unwrap()
            .starts_with("daemon_")
    );
    assert_eq!(batches[0]["operations"][0]["draft_id"], "drf_remote");
    assert_eq!(batches[0]["operations"][0]["expected_draft_version"], 2);
    assert_eq!(batches[0]["operations"][0]["operation"]["action"], "create");
    assert_eq!(
        batches[0]["operations"][0]["operation"]["resource"]["path"],
        "docs/batch.md"
    );
    assert_eq!(
        batches[0]["operations"][0]["operation"]["content"]["content"],
        "two"
    );
    drop(batches);

    let deletes = server.delete_requests.lock().unwrap();
    assert_eq!(
        deletes.as_slice(),
        &[("drf_remote".to_owned(), "3".to_owned())]
    );
}

#[tokio::test]
async fn draft_operation_rejects_multiple_operation_variants() {
    let (_root, _state, service) = common::test_daemon().await;

    let response = service
        .store_draft_operation(DaemonDraftOperationRequest {
            draft_id: None,
            base_commit_id: None,
            project_id: "prj_test".to_owned(),
            scope: DaemonDraftScope::Org,
            resource: DaemonDraftResourceKind::Memory,
            op: DaemonDraftOperation {
                create: Some(DaemonCreateDraftOperation {
                    path: "rules/a.md".to_owned(),
                    content: rule_content("Rule"),
                    description: None,
                }),
                update: None,
                rename: None,
                delete: Some(DaemonDeleteDraftOperation {
                    id: "rule_1".to_owned(),
                    description: None,
                }),
                discard: None,
            },
            source: None,
        })
        .await;

    assert!(matches!(response, Err(DaemonError::InvalidRequest(_))));
}

#[tokio::test]
async fn invalid_commit_payload_does_not_advance_the_installed_ref() {
    let server = AtomicCommitServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = server.url.clone();
    config.project.project_id = Some("prj_atomic".to_owned());
    let (state, _) =
        common::initialize_authenticated_daemon(config, "fake-access-token", None).await;
    let service = DaemonIpcService::new(state.clone());

    service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap();
    let installed = service
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: "prj_atomic".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(installed.state, DaemonMemoryCacheState::Ready);
    assert_eq!(installed.commit_id.as_deref(), Some("commit-valid"));
    let installed_root = installed.active_generation_path.unwrap();
    assert_eq!(
        std::fs::read_to_string(
            std::path::Path::new(&installed_root).join("cache/memory/context/valid.md")
        )
        .unwrap(),
        "Valid authority"
    );

    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", state.local_db_path().display()))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO daemon_meta (key, value)
         VALUES ('commit_sync_last_error:prj_atomic', 'previous scoped failure')",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    server.publish_invalid_commit();
    let error = service
        .retry_sync(DaemonSyncRetryRequest {
            channel: SyncRetryChannel::Commits,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("content-address verification"));

    let after_failure = service
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: "prj_atomic".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(after_failure.state, DaemonMemoryCacheState::Ready);
    assert_eq!(after_failure.commit_id.as_deref(), Some("commit-valid"));
    assert_eq!(
        after_failure.active_generation_path.as_deref(),
        Some(installed_root.as_str())
    );
    assert_eq!(
        std::fs::read_to_string(
            std::path::Path::new(&installed_root).join("cache/memory/context/valid.md")
        )
        .unwrap(),
        "Valid authority"
    );
    assert!(
        !std::path::Path::new(&installed_root)
            .parent()
            .unwrap()
            .join("commit-invalid")
            .exists()
    );

    let sync = service.sync_status().await.unwrap();
    assert_eq!(sync.commit_sync.state, SyncState::Failed);
    assert_eq!(
        sync.commit_sync.server_cursor.as_deref(),
        Some("commit-valid")
    );
    assert!(sync.commit_sync.last_error.is_some());
    assert!(
        service
            .project_sync_status(DaemonProjectSyncStatusRequest {
                project_id: "prj_atomic".to_owned(),
            })
            .await
            .unwrap()
            .commit_sync
            .last_error
            .unwrap()
            .message
            .contains("failed content-address verification")
    );

    std::fs::remove_file(std::path::Path::new(&installed_root).join("manifest.json")).unwrap();
    let corrupted = service
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: "prj_atomic".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(corrupted.state, DaemonMemoryCacheState::GenerationCorrupt);
    assert_eq!(corrupted.commit_id.as_deref(), Some("commit-valid"));
    assert_eq!(corrupted.active_generation_path, None);
    let corrupt_activation = service
        .activate_memory(ActivateMemoryRequest {
            project_id: "prj_atomic".to_owned(),
            query: "authority".to_owned(),
            state: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        corrupt_activation,
        DaemonError::State {
            code: "commit_generation_corrupt",
            ..
        }
    ));

    std::fs::remove_dir_all(&installed_root).unwrap();
    let missing = service
        .memory_cache(DaemonMemoryCacheRequest {
            project_id: "prj_atomic".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(missing.state, DaemonMemoryCacheState::GenerationMissing);
    let missing_activation = service
        .activate_memory(ActivateMemoryRequest {
            project_id: "prj_atomic".to_owned(),
            query: "authority".to_owned(),
            state: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        missing_activation,
        DaemonError::State {
            code: "commit_generation_missing",
            ..
        }
    ));
}

async fn wait_for_create_request(server: &FakeServer) {
    for _ in 0..50 {
        if !server.create_requests.lock().unwrap().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for fake Server create request");
}

async fn wait_for_draft_sync_idle(service: &DaemonIpcService) -> daemon::DaemonSyncStatus {
    for _ in 0..50 {
        let status = service.sync_status().await.unwrap();
        if status.pending_operation_count == 0 {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for daemon draft sync");
}

async fn wait_for_operation_status(
    service: &DaemonIpcService,
    draft_id: &str,
    expected: DraftOperationSyncStatus,
) -> daemon::DaemonDraftDetail {
    for _ in 0..100 {
        let detail = service.get_draft(draft_id).await.unwrap();
        if detail
            .operations
            .iter()
            .any(|operation| operation.sync_status == expected)
        {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for draft operation status {expected:?}");
}

#[derive(Clone)]
struct RecoveringServerState {
    available: Arc<AtomicBool>,
    create_request_count: Arc<AtomicUsize>,
}

async fn recovering_create_draft(
    axum::extract::State(state): axum::extract::State<RecoveringServerState>,
    Json(_body): Json<serde_json::Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    state.create_request_count.fetch_add(1, Ordering::AcqRel);
    if !state.available.load(Ordering::Acquire) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "temporarily unavailable" })),
        )
            .into_response();
    }
    Json(json!({
        "draft": {
            "draft_id": "drf_recovered",
            "version": 1
        }
    }))
    .into_response()
}

async fn empty_draft_events() -> Json<serde_json::Value> {
    Json(json!({
        "events": [],
        "next_cursor": null,
        "has_more": false
    }))
}

#[derive(Clone, Default)]
struct RefreshProbeState {
    stale_requests: Arc<AtomicUsize>,
    fresh_requests: Arc<AtomicUsize>,
    refresh_requests: Arc<AtomicUsize>,
    replacement_requests: Arc<AtomicUsize>,
    both_stale: Arc<Notify>,
    refresh_started: Arc<Notify>,
    release_refresh: Arc<Notify>,
}

fn refresh_probe_server_request() -> DaemonServerRequest {
    DaemonServerRequest {
        method: "POST".to_owned(),
        path: "/api/v1/refresh-probe".to_owned(),
        headers: BTreeMap::new(),
        body: Some("{}".to_owned()),
    }
}

async fn refresh_probe_request(
    axum::extract::State(state): axum::extract::State<RefreshProbeState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    match proxy_header_value(&headers, "authorization").as_deref() {
        Some("Bearer stale-token") => {
            if state.stale_requests.fetch_add(1, Ordering::AcqRel) == 1 {
                state.both_stale.notify_one();
            }
            (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "expired" })),
            )
                .into_response()
        }
        Some("Bearer fresh-token") => {
            state.fresh_requests.fetch_add(1, Ordering::AcqRel);
            Json(json!({ "ok": true })).into_response()
        }
        Some("Bearer replacement-token") => {
            state.replacement_requests.fetch_add(1, Ordering::AcqRel);
            Json(json!({ "ok": true })).into_response()
        }
        _ => (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({ "error": "unexpected credential" })),
        )
            .into_response(),
    }
}

async fn refresh_probe_token(
    axum::extract::State(state): axum::extract::State<RefreshProbeState>,
) -> Json<serde_json::Value> {
    state.refresh_requests.fetch_add(1, Ordering::AcqRel);
    state.refresh_started.notify_one();
    state.release_refresh.notified().await;
    Json(json!({
        "access_token": "fresh-token",
        "refresh_token": "rotated-refresh-token"
    }))
}

#[derive(Clone)]
struct CachedProxyState {
    unavailable: Arc<AtomicBool>,
}

async fn wait_for_server_response_cache(state: &DaemonState, path: &str, expected_body: &str) {
    let server_url = state.project_config_status().server_url;
    let pool = sqlx::SqlitePool::connect(&state.local_db_path().display().to_string())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let body: Option<String> = sqlx::query_scalar(
                "SELECT body FROM server_response_cache WHERE server_url = $1 AND path = $2",
            )
            .bind(&server_url)
            .bind(path)
            .fetch_optional(&pool)
            .await
            .unwrap();
            if body.as_deref() == Some(expected_body) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("live Server response should eventually reach the stale cache");
    pool.close().await;
}

#[derive(Deserialize)]
struct ProxyHeaderProbe {
    idempotency_key: Option<String>,
    if_match: Option<String>,
    if_none_match: Option<String>,
    authorization: Vec<String>,
    cookie: Option<String>,
    untrusted: Option<String>,
}

async fn proxy_header_probe(headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
    let authorization = headers
        .get_all("authorization")
        .iter()
        .filter_map(|value| value.to_str().ok().map(str::to_owned))
        .collect::<Vec<_>>();
    Json(json!({
        "idempotency_key": proxy_header_value(&headers, "idempotency-key"),
        "if_match": proxy_header_value(&headers, "if-match"),
        "if_none_match": proxy_header_value(&headers, "if-none-match"),
        "authorization": authorization,
        "cookie": proxy_header_value(&headers, "cookie"),
        "untrusted": proxy_header_value(&headers, "x-untrusted"),
    }))
}

fn proxy_header_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

async fn cached_proxy_get(
    axum::extract::State(state): axum::extract::State<CachedProxyState>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if state.unavailable.load(Ordering::Acquire) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "temporarily unavailable" })),
        )
            .into_response();
    }
    Json(json!({ "source": "live" })).into_response()
}

async fn cached_proxy_post() -> axum::response::Response {
    use axum::response::IntoResponse;

    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "temporarily unavailable" })),
    )
        .into_response()
}

async fn cached_missing_get(
    axum::extract::State(state): axum::extract::State<CachedProxyState>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if state.unavailable.load(Ordering::Acquire) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "temporarily unavailable" })),
        )
            .into_response();
    }
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(json!({ "error": "not found" })),
    )
        .into_response()
}

struct FakeServer {
    url: String,
    create_requests: Arc<Mutex<Vec<serde_json::Value>>>,
    batch_requests: Arc<Mutex<Vec<serde_json::Value>>>,
    delete_requests: Arc<Mutex<Vec<(String, String)>>>,
}

#[derive(Clone)]
struct BlockingDraftEventsState {
    started: Arc<Notify>,
    release: Arc<Notify>,
    request_count: Arc<AtomicUsize>,
}

async fn blocking_draft_events(
    axum::extract::State(state): axum::extract::State<BlockingDraftEventsState>,
) -> Json<serde_json::Value> {
    let request_index = state.request_count.fetch_add(1, Ordering::AcqRel);
    if request_index == 0 {
        state.started.notify_one();
        state.release.notified().await;
    }
    Json(json!({
        "events": [],
        "next_cursor": null,
        "has_more": false
    }))
}

#[derive(Clone)]
struct TerminalDraftProjectionState {
    request_count: Arc<AtomicUsize>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
}

async fn terminal_draft_events() -> Json<serde_json::Value> {
    let events = (0..12)
        .map(|index| {
            json!({
                "event_id": format!("evt_terminal_{index}"),
                "draft_id": format!("drf_terminal_{index}"),
                "project_id": "prj_terminal",
                "event_type": "merged",
                "version": 1,
                "daemon_installation_id": null,
                "created_at": "2026-08-25T00:00:00Z"
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "events": events,
        "next_cursor": "terminal:12",
        "has_more": false
    }))
}

async fn terminal_draft_projection(
    axum::extract::State(state): axum::extract::State<TerminalDraftProjectionState>,
    axum::extract::Path(draft_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    state.request_count.fetch_add(1, Ordering::AcqRel);
    let in_flight = state.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
    state.max_in_flight.fetch_max(in_flight, Ordering::AcqRel);
    tokio::time::sleep(Duration::from_millis(40)).await;
    state.in_flight.fetch_sub(1, Ordering::AcqRel);

    Json(json!({
        "draft": {
            "draft_id": draft_id,
            "project_id": "prj_terminal",
            "base_commit_id": COMMIT_A,
            "resource": {
                "scope": "project",
                "id": "ctx_terminal",
                "path": "docs/terminal.md"
            },
            "coordination": {
                "current_commit_id": COMMIT_A,
                "freshness": "current",
                "has_upstream_resource_changes": false,
                "reconciliation": "unknown",
                "candidate_id": null
            },
            "status": "merged",
            "version": 1,
            "created_at": "2026-08-25T00:00:00Z",
            "updated_at": "2026-08-25T00:00:00Z"
        },
        "operations": []
    }))
}

struct AtomicCommitServer {
    url: String,
    revision: Arc<AtomicUsize>,
}

impl AtomicCommitServer {
    async fn start() -> Self {
        let revision = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/api/v1/me", get(atomic_identity))
            .route("/api/v1/org/commit-state", get(atomic_org_commit_state))
            .route(
                "/api/v1/projects/{project_id}/commit-state",
                get(atomic_project_commit_state),
            )
            .route("/api/v1/commits/{commit_id}", get(atomic_commit_payload))
            .with_state(revision.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            url: format!("http://{addr}"),
            revision,
        }
    }

    fn publish_invalid_commit(&self) {
        self.revision.store(1, Ordering::Release);
    }
}

#[derive(Deserialize)]
struct AtomicCommitStateQuery {
    local_commit_id: Option<String>,
}

async fn atomic_org_commit_state() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::ETAG, "\"ref-none\"")],
        Json(json!({
            "update_available": false,
            "ref": {
                "name": "refs/heads/main",
                "scope": "org",
                "org_id": "org_atomic",
                "project_id": null,
                "commit_id": null,
                "updated_at": "2026-07-15T00:00:00Z"
            },
            "latest": null,
            "download_url": null,
            "incremental_supported": false
        })),
    )
}

async fn atomic_project_commit_state(
    axum::extract::State(revision): axum::extract::State<Arc<AtomicUsize>>,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<AtomicCommitStateQuery>,
) -> impl axum::response::IntoResponse {
    let invalid = revision.load(Ordering::Acquire) == 1;
    let commit_id = if invalid {
        "commit-invalid"
    } else {
        "commit-valid"
    };
    let tree_id = if invalid {
        "tree-invalid"
    } else {
        "tree-valid"
    };
    let parent_commit_id = invalid.then_some("commit-valid");
    let version = if invalid { 2 } else { 1 };
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::ETAG,
        axum::http::HeaderValue::from_str(&format!("\"{commit_id}\"")).unwrap(),
    );
    (
        headers,
        Json(json!({
            "update_available": query.local_commit_id.as_deref() != Some(commit_id),
            "ref": {
                "name": "refs/heads/main",
                "scope": "project",
                "org_id": "org_atomic",
                "project_id": project_id,
                "commit_id": commit_id,
                "updated_at": "2026-07-15T00:00:00Z"
            },
            "latest": {
                "commit_id": commit_id,
                "scope": "project",
                "org_id": "org_atomic",
                "project_id": "prj_atomic",
                "tree_id": tree_id,
                "parent_commit_id": parent_commit_id,
                "version": version,
                "created_at": "2026-07-15T00:00:00Z"
            },
            "download_url": format!("/api/v1/commits/{commit_id}"),
            "incremental_supported": false
        })),
    )
}

async fn atomic_commit_payload(
    axum::extract::Path(commit_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let invalid = commit_id == "commit-invalid";
    let tree_id = if invalid {
        "tree-invalid"
    } else {
        "tree-valid"
    };
    let path = if invalid {
        "context/invalid.md"
    } else {
        "context/valid.md"
    };
    let content = if invalid {
        "Invalid authority"
    } else {
        "Valid authority"
    };
    let content_blob_id = if invalid {
        "not-the-content-address".to_owned()
    } else {
        test_blob_id(content)
    };
    let selection = json!({
        "project_id": "prj_atomic",
        "memories": [],
        "revision": 0
    });
    let selection_content = serde_json::to_string(&selection).unwrap();
    let selection_blob_id = test_blob_id(&selection_content);
    Json(json!({
        "commit": {
            "commit_id": commit_id,
            "scope": "project",
            "org_id": "org_atomic",
            "project_id": "prj_atomic",
            "tree_id": tree_id,
            "parent_commit_id": invalid.then_some("commit-valid"),
            "version": if invalid { 2 } else { 1 },
            "created_at": "2026-07-15T00:00:00Z"
        },
        "tree": {
            "tree_id": tree_id,
            "entries": [
                {
                    "id": if invalid { "ctx_invalid" } else { "ctx_valid" },
                    "type": "context",
                    "scope": "project",
                    "project_id": "prj_atomic",
                    "path": path,
                    "blob_id": content_blob_id,
                    "source": "project"
                },
                {
                    "id": "project_org_selection",
                    "type": "project_org_selection",
                    "scope": "daemon",
                    "project_id": "prj_atomic",
                    "path": null,
                    "blob_id": selection_blob_id,
                    "source": "config"
                }
            ]
        },
        "blobs": [
            { "blob_id": content_blob_id, "content": content },
            { "blob_id": selection_blob_id, "content": selection_content }
        ],
        "project_org_selection": selection
    }))
}

fn test_blob_id(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"blob\0");
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

impl FakeServer {
    async fn start() -> Self {
        let create_requests = Arc::new(Mutex::new(Vec::new()));
        let batch_requests = Arc::new(Mutex::new(Vec::new()));
        let delete_requests = Arc::new(Mutex::new(Vec::new()));
        let state = FakeServerState {
            create_requests: create_requests.clone(),
            batch_requests: batch_requests.clone(),
            delete_requests: delete_requests.clone(),
        };
        let app = Router::new()
            .route("/api/v1/drafts", post(fake_create_draft))
            .route(
                "/api/v1/draft-events",
                get(fake_list_draft_events_after_create),
            )
            .route("/api/v1/me", get(fake_identity))
            .route("/api/v1/org/commit-state", get(fake_org_commit_state))
            .route(
                "/api/v1/projects/{project_id}/commit-state",
                get(fake_project_commit_state),
            )
            .route(
                "/api/v1/drafts/{draft_id}",
                get(fake_get_draft).delete(fake_delete_draft),
            )
            .route(
                "/api/v1/draft-operation-batches",
                post(fake_create_draft_operation_batch),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            url: format!("http://{addr}"),
            create_requests,
            batch_requests,
            delete_requests,
        }
    }
}

async fn fake_org_commit_state() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::ETAG, "\"ref-none\"")],
        Json(json!({
            "update_available": false,
            "ref": {
                "name": "refs/heads/main",
                "scope": "org",
                "org_id": "org_test",
                "project_id": null,
                "commit_id": null,
                "updated_at": "2026-07-08T00:00:00Z"
            },
            "latest": null,
            "download_url": null,
            "incremental_supported": false
        })),
    )
}

async fn fake_project_commit_state(
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::ETAG, "\"ref-none\"")],
        Json(json!({
            "update_available": false,
            "ref": {
                "name": "refs/heads/main",
                "scope": "project",
                "org_id": "org_test",
                "project_id": project_id,
                "commit_id": null,
                "updated_at": "2026-07-08T00:00:00Z"
            },
            "latest": null,
            "download_url": null,
            "incremental_supported": false
        })),
    )
}

#[derive(Clone)]
struct FakeServerState {
    create_requests: Arc<Mutex<Vec<serde_json::Value>>>,
    batch_requests: Arc<Mutex<Vec<serde_json::Value>>>,
    delete_requests: Arc<Mutex<Vec<(String, String)>>>,
}

async fn fake_create_draft(
    axum::extract::State(state): axum::extract::State<FakeServerState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    state.create_requests.lock().unwrap().push(body);
    Json(json!({
        "draft": {
            "draft_id": "drf_remote",
            "version": 1
        }
    }))
}

async fn fake_create_draft_operation_batch(
    axum::extract::State(state): axum::extract::State<FakeServerState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let accepted_operations = body["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|operation| operation["local_operation_id"].clone())
        .collect::<Vec<_>>();
    state.batch_requests.lock().unwrap().push(body);
    Json(json!({
        "accepted_operations": accepted_operations,
        "cursor": "2"
    }))
}

async fn fake_get_draft(
    axum::extract::State(state): axum::extract::State<FakeServerState>,
    axum::extract::Path(draft_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let create_request = state
        .create_requests
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "project_id": "prj_test",
                "base_commit_id": null,
                "resource": {
                    "scope": "project",
                    "id": null,
                    "path": "docs/remote.md"
                },
                "operations": [{
                    "action": "create",
                    "resource": {
                        "scope": "project",
                        "id": null,
                        "path": "docs/remote.md"
                    },
                    "content": {
                        "content": "Remote base"
                    },
                    "new_path": null
                }]
            })
        });
    let resource = create_request["resource"].clone();
    let initial_operation = create_request["operations"][0].clone();
    Json(json!({
        "draft": {
            "draft_id": draft_id,
            "project_id": create_request["project_id"],
            "base_commit_id": create_request["base_commit_id"],
            "resource": resource,
            "coordination": {
                "current_commit_id": create_request["base_commit_id"],
                "freshness": "current",
                "has_upstream_resource_changes": false,
                "reconciliation": "unknown",
                "candidate_id": null
            },
            "status": "open",
            "version": 2,
            "created_at": "2026-07-08T00:00:00Z",
            "updated_at": "2026-07-08T00:01:00Z"
        },
        "operations": [
            {
                "operation_id": "dop_initial",
                "action": initial_operation["action"],
                "resource": initial_operation["resource"],
                "content": initial_operation["content"],
                "new_path": initial_operation["new_path"],
                "created_at": "2026-07-08T00:00:00Z"
            },
            {
                "operation_id": "dop_remote",
                "action": "create",
                "resource": create_request["resource"],
                "content": {
                    "content": "Remote revision"
                },
                "new_path": null,
                "created_at": "2026-07-08T00:01:00Z"
            }
        ],
        "sync_state": {
            "status": "synced",
            "server_cursor": "draft:drf_remote:2",
            "daemon_installation_id": "daemon_other",
            "conflict_count": 0
        },
        "conflict": null
    }))
}

async fn fake_stale_draft(
    axum::extract::Path(draft_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    Json(json!({
        "draft": {
            "draft_id": draft_id,
            "project_id": "prj_test",
            "base_commit_id": null,
            "resource": {
                "scope": "project",
                "id": null,
                "path": "docs/remote.md"
            },
            "coordination": {
                "current_commit_id": null,
                "freshness": "current",
                "has_upstream_resource_changes": false,
                "reconciliation": "unknown",
                "candidate_id": null
            },
            "status": "open",
            "version": 1,
            "created_at": "2026-07-08T00:00:00Z",
            "updated_at": "2026-07-08T00:00:00Z"
        },
        "operations": [],
        "conflict": null
    }))
}

async fn fake_delete_draft(
    axum::extract::State(state): axum::extract::State<FakeServerState>,
    axum::extract::Path(draft_id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    let version = headers
        .get("if-match")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    state
        .delete_requests
        .lock()
        .unwrap()
        .push((draft_id.clone(), version));
    Json(json!({ "deleted": true, "id": draft_id }))
}

#[derive(Deserialize)]
struct FakeDraftEventQuery {
    after_cursor: Option<String>,
}

#[derive(Clone)]
struct FakeConflictProjectionState {
    resolved: Arc<AtomicUsize>,
}

async fn fake_list_draft_events(
    axum::extract::Query(query): axum::extract::Query<FakeDraftEventQuery>,
) -> Json<serde_json::Value> {
    fake_draft_events_response(query)
}

async fn fake_list_draft_events_after_create(
    axum::extract::State(state): axum::extract::State<FakeServerState>,
    axum::extract::Query(query): axum::extract::Query<FakeDraftEventQuery>,
) -> Json<serde_json::Value> {
    if state.create_requests.lock().unwrap().is_empty() {
        return Json(json!({
            "events": [],
            "next_cursor": null,
            "has_more": false
        }));
    }
    fake_draft_events_response(query)
}

fn fake_draft_events_response(query: FakeDraftEventQuery) -> Json<serde_json::Value> {
    if query.after_cursor.is_some() {
        return Json(json!({
            "events": [],
            "next_cursor": null,
            "has_more": false
        }));
    }
    Json(json!({
        "events": [
            {
                "event_id": "evt_remote",
                "draft_id": "drf_remote",
                "project_id": "prj_test",
                "event_type": "updated",
                "version": 2,
                "daemon_installation_id": "daemon_other",
                "created_at": "2026-07-08T00:00:00Z"
            }
        ],
        "next_cursor": "42",
        "has_more": false
    }))
}

async fn fake_conflicted_draft_events(
    axum::extract::State(state): axum::extract::State<FakeConflictProjectionState>,
    axum::extract::Query(query): axum::extract::Query<FakeDraftEventQuery>,
) -> Json<serde_json::Value> {
    if query.after_cursor.as_deref() == Some("conflict:3")
        && state.resolved.load(Ordering::SeqCst) > 0
    {
        return Json(json!({
            "events": [
                {
                    "event_id": "evt_resolved",
                    "draft_id": "drf_conflict",
                    "project_id": "prj_conflict",
                    "event_type": "updated",
                    "version": 4,
                    "daemon_installation_id": null,
                    "created_at": "2026-07-15T00:02:00Z"
                }
            ],
            "next_cursor": "resolved:4",
            "has_more": false
        }));
    }
    if query.after_cursor.is_some() {
        return Json(json!({
            "events": [],
            "next_cursor": null,
            "has_more": false
        }));
    }
    Json(json!({
        "events": [
            {
                "event_id": "evt_conflict",
                "draft_id": "drf_conflict",
                "project_id": "prj_conflict",
                    "event_type": "updated",
                "version": 3,
                "daemon_installation_id": null,
                "created_at": "2026-07-15T00:00:00Z"
            }
        ],
        "next_cursor": "conflict:3",
        "has_more": false
    }))
}

async fn fake_conflicted_draft(
    axum::extract::State(state): axum::extract::State<FakeConflictProjectionState>,
    axum::extract::Path(draft_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    if state.resolved.load(Ordering::SeqCst) > 0 {
        return Json(json!({
            "draft": {
                "draft_id": draft_id,
                "project_id": "prj_conflict",
                "base_commit_id": COMMIT_B,
                "resource": {
                    "scope": "project",
                    "id": "ctx_conflict",
                    "path": "docs/conflict.md"
                },
                "coordination": {
                    "current_commit_id": COMMIT_B,
                    "freshness": "current",
                    "has_upstream_resource_changes": false,
                    "reconciliation": "unknown",
                    "candidate_id": null
                },
                "status": "submitted",
                "version": 4,
                "created_at": "2026-07-15T00:00:00Z",
                "updated_at": "2026-07-15T00:02:00Z"
            },
            "operations": [
                {
                    "operation_id": "dop_resolved",
                    "action": "update",
                    "resource": {
                        "scope": "project",
                        "id": "ctx_conflict",
                        "path": "docs/conflict.md"
                    },
                    "content": {
                        "content": "Resolved content"
                    },
                    "new_path": null,
                    "created_at": "2026-07-15T00:02:00Z"
                }
            ]
        }));
    }
    Json(json!({
        "draft": {
            "draft_id": draft_id,
            "project_id": "prj_conflict",
            "base_commit_id": COMMIT_A,
            "resource": {
                "scope": "project",
                "id": "ctx_conflict",
                "path": "docs/conflict.md"
            },
            "coordination": {
                "current_commit_id": COMMIT_B,
                "freshness": "behind",
                "has_upstream_resource_changes": true,
                "reconciliation": "conflicts",
                "candidate_id": "rcn_conflict"
            },
            "status": "submitted",
            "version": 3,
            "created_at": "2026-07-15T00:00:00Z",
            "updated_at": "2026-07-15T00:01:00Z"
        },
        "operations": [
            {
                "operation_id": "dop_conflict",
                "action": "update",
                "resource": {
                    "scope": "project",
                    "id": "ctx_conflict",
                    "path": "docs/conflict.md"
                },
                "content": {
                    "content": "Draft content"
                },
                "new_path": null,
                "created_at": "2026-07-15T00:00:30Z"
            }
        ]
    }))
}
