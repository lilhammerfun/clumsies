use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use reqwest::header::ETAG;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use super::*;

const META_COMMIT_SYNC_LAST_ATTEMPT_AT: &str = "commit_sync_last_attempt_at";
const META_COMMIT_SYNC_LAST_SUCCESS_AT: &str = "commit_sync_last_success_at";
const META_COMMIT_SYNC_LAST_ERROR: &str = "commit_sync_last_error";
const MAIN_REF: &str = "refs/heads/main";
const EMPTY_GENERATION: &str = "ref-none";
const MAX_CONCURRENT_PROJECT_STATE_REQUESTS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommitSyncRunScope {
    Idle,
    Global,
    Project(String),
}

impl CommitSyncRunScope {
    fn new(project_id: Option<&str>) -> Self {
        match project_id {
            Some(project_id) => Self::Project(project_id.to_owned()),
            None => Self::Global,
        }
    }

    fn is_running_for(&self, project_id: Option<&str>) -> bool {
        match (self, project_id) {
            (Self::Idle, _) => false,
            (_, None) | (Self::Global, Some(_)) => true,
            (Self::Project(running_project_id), Some(project_id)) => {
                running_project_id == project_id
            }
        }
    }
}

struct CommitSyncOutcome {
    attempted_project_ids: BTreeSet<String>,
    project_errors: BTreeMap<String, String>,
    error: Option<DaemonError>,
}

impl CommitSyncOutcome {
    fn from_project_errors(
        attempted_project_ids: BTreeSet<String>,
        errors: BTreeMap<String, DaemonError>,
    ) -> Self {
        let project_errors = errors
            .iter()
            .map(|(project_id, error)| (project_id.clone(), error.to_string()))
            .collect();
        Self {
            attempted_project_ids,
            project_errors,
            error: errors.into_values().next(),
        }
    }

    fn from_shared_error(
        attempted_project_ids: BTreeSet<String>,
        errors: BTreeMap<String, DaemonError>,
        shared_error: DaemonError,
    ) -> Self {
        Self::from_shared_error_inner(attempted_project_ids, errors, shared_error, false)
    }

    fn from_shared_error_after_project_errors(
        attempted_project_ids: BTreeSet<String>,
        errors: BTreeMap<String, DaemonError>,
        shared_error: DaemonError,
    ) -> Self {
        Self::from_shared_error_inner(attempted_project_ids, errors, shared_error, true)
    }

    fn from_shared_error_inner(
        attempted_project_ids: BTreeSet<String>,
        errors: BTreeMap<String, DaemonError>,
        shared_error: DaemonError,
        project_error_first: bool,
    ) -> Self {
        let shared_message = shared_error.to_string();
        let mut project_errors = errors
            .iter()
            .map(|(project_id, error)| (project_id.clone(), error.to_string()))
            .collect::<BTreeMap<_, _>>();
        for project_id in &attempted_project_ids {
            project_errors
                .entry(project_id.clone())
                .or_insert_with(|| shared_message.clone());
        }
        let error = if project_error_first {
            errors.into_values().next().unwrap_or(shared_error)
        } else {
            shared_error
        };
        Self {
            attempted_project_ids,
            project_errors,
            error: Some(error),
        }
    }

    fn into_result(self) -> Result<(), DaemonError> {
        match self.error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonMemoryCacheRequest {
    pub project_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonMemoryCacheStatus {
    pub project_id: String,
    pub commit_id: Option<String>,
    pub active_generation_path: Option<String>,
    pub state: DaemonMemoryCacheState,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonMemoryCacheState {
    ProjectRefNotSynced,
    StorageUnavailable,
    GenerationMissing,
    GenerationCorrupt,
    Ready,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectCheckoutRequest {
    pub project_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectCheckout {
    pub project_id: String,
    pub commit_id: Option<String>,
    pub ref_etag: Option<String>,
    pub commit_created_at: Option<String>,
    pub org_selection_revision: i64,
    pub selected_org_resource_ids: Vec<String>,
    pub resources: Vec<DaemonProjectCheckoutResource>,
    pub ready: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectCheckoutResource {
    pub resource_id: String,
    pub scope: DaemonDraftScope,
    pub resource_kind: DaemonDraftResourceKind,
    pub project_id: Option<String>,
    pub path: String,
    pub content_hash: String,
    pub content: DaemonDraftContent,
}

pub(super) async fn migrate(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cached_blobs (
            blob_id TEXT PRIMARY KEY,
            content TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cached_trees (
            tree_id TEXT PRIMARY KEY,
            payload_json TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cached_commits (
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
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cached_refs (
            ref_key TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
            org_id TEXT NOT NULL,
            project_id TEXT,
            commit_id TEXT,
            etag TEXT NOT NULL,
            server_updated_at TEXT NOT NULL,
            installed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_cached_refs_project_id
         ON cached_refs (project_id)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn run(state: &DaemonState) -> Result<(), DaemonError> {
    run_scoped(state, None).await
}

pub(super) async fn run_for_project(
    state: &DaemonState,
    project_id: &str,
) -> Result<(), DaemonError> {
    validate_cache_component("project_id", project_id)?;
    run_scoped(state, Some(project_id)).await
}

async fn run_scoped(state: &DaemonState, project_id: Option<&str>) -> Result<(), DaemonError> {
    *state
        .inner
        .commit_sync_run_scope
        .write()
        .expect("commit sync scope rwlock poisoned") = CommitSyncRunScope::new(project_id);
    let last_attempt_key = commit_sync_meta_key(META_COMMIT_SYNC_LAST_ATTEMPT_AT, project_id);
    let last_error_key = commit_sync_meta_key(META_COMMIT_SYNC_LAST_ERROR, project_id);
    let result = async {
        upsert_meta_timestamp(&state.inner.pool, &last_attempt_key).await?;
        let outcome = match project_id {
            Some(project_id) => sync_refs(state, BTreeSet::from([project_id.to_owned()])).await,
            None => match sync_project_ids(state).await {
                Ok(project_ids) => {
                    record_project_attempts(&state.inner.pool, &project_ids).await?;
                    sync_refs(state, project_ids).await
                }
                Err(error) => {
                    CommitSyncOutcome::from_shared_error(BTreeSet::new(), BTreeMap::new(), error)
                }
            },
        };
        let mut tx = state.inner.pool.begin().await?;
        match &outcome.error {
            None => {
                upsert_meta_value(&mut tx, &last_error_key, None).await?;
                if project_id.is_none() {
                    sqlx::query(
                        "DELETE FROM daemon_meta
                         WHERE key GLOB 'commit_sync_last_error:*'",
                    )
                    .execute(&mut *tx)
                    .await?;
                }
            }
            Some(error) => {
                upsert_meta_value(&mut tx, &last_error_key, Some(&error.to_string())).await?;
                if project_id.is_none() {
                    for attempted_project_id in &outcome.attempted_project_ids {
                        let key = commit_sync_meta_key(
                            META_COMMIT_SYNC_LAST_ERROR,
                            Some(attempted_project_id),
                        );
                        upsert_meta_value(
                            &mut tx,
                            &key,
                            outcome
                                .project_errors
                                .get(attempted_project_id)
                                .map(String::as_str),
                        )
                        .await?;
                    }
                }
            }
        }
        tx.commit().await?;
        if project_id.is_none() && outcome.error.is_none() {
            upsert_meta_timestamp(&state.inner.pool, META_COMMIT_SYNC_LAST_SUCCESS_AT).await?;
        }
        outcome.into_result()
    }
    .await;
    *state
        .inner
        .commit_sync_run_scope
        .write()
        .expect("commit sync scope rwlock poisoned") = CommitSyncRunScope::Idle;
    result
}

/// Record a failed identity preflight without treating it as an empty project list.
///
/// # Errors
/// Propagates failures to persist the synchronization diagnostic.
pub(crate) async fn record_sync_error(
    state: &DaemonState,
    project_id: Option<&str>,
    error: &DaemonError,
) -> Result<(), DaemonError> {
    let key = commit_sync_meta_key(META_COMMIT_SYNC_LAST_ERROR, project_id);
    let mut tx = state.inner.pool.begin().await?;
    upsert_meta_value(&mut tx, &key, Some(&error.to_string())).await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn status(state: &DaemonState) -> Result<SyncChannelStatus, DaemonError> {
    status_scoped(state, None).await
}

pub(super) async fn status_for_project(
    state: &DaemonState,
    project_id: &str,
) -> Result<SyncChannelStatus, DaemonError> {
    validate_cache_component("project_id", project_id)?;
    status_scoped(state, Some(project_id)).await
}

async fn status_scoped(
    state: &DaemonState,
    scoped_project_id: Option<&str>,
) -> Result<SyncChannelStatus, DaemonError> {
    let config = state.project_config();
    let readiness = config.server_readiness();
    let project_id = scoped_project_id.or(config.project_id.as_deref());
    let (server_cursor, installed_at) = match project_id {
        Some(project_id) => load_project_ref_status(&state.inner.pool, project_id).await?,
        None => (None, None),
    };
    let ref_installed = installed_at.is_some();
    let last_attempt_key =
        commit_sync_meta_key(META_COMMIT_SYNC_LAST_ATTEMPT_AT, scoped_project_id);
    let last_error_key = commit_sync_meta_key(META_COMMIT_SYNC_LAST_ERROR, scoped_project_id);
    let last_attempt_at = load_meta_value(&state.inner.pool, &last_attempt_key).await?;
    let last_success_at = match scoped_project_id {
        Some(_) => installed_at,
        None => load_meta_value(&state.inner.pool, META_COMMIT_SYNC_LAST_SUCCESS_AT).await?,
    };
    let last_error = load_meta_value(&state.inner.pool, &last_error_key).await?;

    let config_error = (!readiness.ready && state.inner.config.sync.enabled).then(|| ApiError {
        code: "daemon_project_config_incomplete".to_owned(),
        message: format!(
            "Daemon project config is missing required fields: {}",
            readiness.missing_fields.join(", ")
        ),
        request_id: "local".to_owned(),
        details: json!({ "missing_fields": readiness.missing_fields }),
    });
    let running_scope = state
        .inner
        .commit_sync_run_scope
        .read()
        .expect("commit sync scope rwlock poisoned")
        .clone();
    let running = running_scope.is_running_for(scoped_project_id);
    let state_value = if running {
        SyncState::Syncing
    } else if config_error.is_some() {
        SyncState::Degraded
    } else if last_error.is_some() {
        SyncState::Failed
    } else if readiness.ready
        && !ref_installed
        && !(project_id.is_none() && last_success_at.is_some())
    {
        SyncState::Queued
    } else {
        SyncState::Idle
    };

    Ok(SyncChannelStatus {
        state: state_value,
        server_cursor,
        last_attempt_at,
        last_success_at,
        last_error: config_error.or_else(|| {
            last_error.map(|message| ApiError {
                code: "commit_sync_failed".to_owned(),
                message,
                request_id: "local".to_owned(),
                details: json!({}),
            })
        }),
    })
}

async fn load_project_ref_status(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<(Option<String>, Option<String>), DaemonError> {
    let row = sqlx::query(
        "SELECT commit_id, installed_at
         FROM cached_refs
         WHERE ref_key = $1 AND scope = 'project'",
    )
    .bind(project_ref_key(project_id))
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok((
            row.try_get("commit_id")?,
            Some(row.try_get("installed_at")?),
        )),
        None => Ok((None, None)),
    }
}

fn commit_sync_meta_key(base: &str, project_id: Option<&str>) -> String {
    match project_id {
        Some(project_id) => format!("{base}:{project_id}"),
        None => base.to_owned(),
    }
}

async fn record_project_attempts(
    pool: &SqlitePool,
    project_ids: &BTreeSet<String>,
) -> Result<(), DaemonError> {
    if project_ids.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    let timestamp: String = sqlx::query_scalar("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')")
        .fetch_one(&mut *tx)
        .await?;
    for project_id in project_ids {
        let key = commit_sync_meta_key(META_COMMIT_SYNC_LAST_ATTEMPT_AT, Some(project_id));
        upsert_meta_value(&mut tx, &key, Some(&timestamp)).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn current_base_commit_id(
    pool: &SqlitePool,
    project_id: &str,
    scope: DaemonDraftScope,
) -> Result<Option<String>, DaemonError> {
    match scope {
        DaemonDraftScope::Project => load_ref_commit(pool, &project_ref_key(project_id)).await,
        DaemonDraftScope::Org => {
            let org_id: Option<String> = sqlx::query_scalar(
                "SELECT org_id FROM cached_refs WHERE ref_key = $1 AND scope = 'project'",
            )
            .bind(project_ref_key(project_id))
            .fetch_optional(pool)
            .await?;
            match org_id {
                Some(org_id) => load_ref_commit(pool, &org_ref_key(&org_id)).await,
                None => Ok(None),
            }
        }
    }
}

pub(super) async fn ensure_commit_cached(
    state: &DaemonState,
    commit_id: &str,
) -> Result<(), DaemonError> {
    validate_cache_component("commit_id", commit_id)?;
    if cached_commit_exists(&state.inner.pool, commit_id).await? {
        return Ok(());
    }
    let payload: ServerCommitPayload =
        get_server_json(state, &format!("/api/v1/commits/{commit_id}")).await?;
    if payload.commit.commit_id != commit_id {
        return Err(DaemonError::Server(format!(
            "Server returned Commit {} while {commit_id} was requested",
            payload.commit.commit_id
        )));
    }
    let synthetic_state = ServerCommitState {
        update_available: false,
        reference: ServerRef {
            name: "refs/heads/base-retention".to_owned(),
            scope: payload.commit.scope,
            org_id: payload.commit.org_id.clone(),
            project_id: payload.commit.project_id.clone(),
            commit_id: Some(payload.commit.commit_id.clone()),
            updated_at: payload.commit.created_at.clone(),
        },
        latest: Some(payload.commit.clone()),
        download_url: Some(format!("/api/v1/commits/{commit_id}")),
        incremental_supported: false,
    };
    validate_commit_payload(&payload, &synthetic_state)?;
    let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
    cache_commit_payload(&mut tx, &payload).await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn memory_cache(
    state: &DaemonState,
    request: DaemonMemoryCacheRequest,
) -> Result<DaemonMemoryCacheStatus, DaemonError> {
    let _storage_guard = state.inner.storage_access.read().await;
    memory_cache_under_storage_guard(state, request).await
}

pub(super) async fn memory_cache_under_storage_guard(
    state: &DaemonState,
    request: DaemonMemoryCacheRequest,
) -> Result<DaemonMemoryCacheStatus, DaemonError> {
    validate_cache_component("project_id", &request.project_id)?;
    let row = sqlx::query(
        "SELECT commit_id
         FROM cached_refs
         WHERE ref_key = $1 AND scope = 'project'",
    )
    .bind(project_ref_key(&request.project_id))
    .fetch_optional(&state.inner.pool)
    .await?;
    let Some(row) = row else {
        return Ok(DaemonMemoryCacheStatus {
            project_id: request.project_id,
            commit_id: None,
            active_generation_path: None,
            state: DaemonMemoryCacheState::ProjectRefNotSynced,
            diagnostic: Some("the Project Ref has not been synchronized".to_owned()),
        });
    };
    let commit_id: Option<String> = row.try_get("commit_id")?;
    let storage = match super::project_storage::resolve_active(state, &request.project_id).await {
        Ok(storage) => storage,
        Err(error) => {
            return Ok(DaemonMemoryCacheStatus {
                project_id: request.project_id,
                commit_id,
                active_generation_path: None,
                state: DaemonMemoryCacheState::StorageUnavailable,
                diagnostic: Some(error.to_string()),
            });
        }
    };
    let generation = storage.generation_path(commit_id.as_deref().unwrap_or(EMPTY_GENERATION));
    if !generation.exists() {
        return Ok(DaemonMemoryCacheStatus {
            project_id: request.project_id,
            commit_id,
            active_generation_path: None,
            state: DaemonMemoryCacheState::GenerationMissing,
            diagnostic: Some(format!(
                "the installed Commit generation {} is missing",
                generation.display()
            )),
        });
    }
    if let Err(error) =
        verify_generation_ref(&generation, &request.project_id, commit_id.as_deref())
    {
        return Ok(DaemonMemoryCacheStatus {
            project_id: request.project_id,
            commit_id,
            active_generation_path: None,
            state: DaemonMemoryCacheState::GenerationCorrupt,
            diagnostic: Some(error.to_string()),
        });
    }
    Ok(DaemonMemoryCacheStatus {
        project_id: request.project_id,
        commit_id,
        active_generation_path: Some(generation.display().to_string()),
        state: DaemonMemoryCacheState::Ready,
        diagnostic: None,
    })
}

pub(super) async fn project_checkout(
    state: &DaemonState,
    request: DaemonProjectCheckoutRequest,
) -> Result<DaemonProjectCheckout, DaemonError> {
    validate_cache_component("project_id", &request.project_id)?;
    let _storage_guard = state.inner.storage_access.read().await;
    let row = sqlx::query(
        "SELECT commit_id, etag
         FROM cached_refs
         WHERE ref_key = $1 AND scope = 'project'",
    )
    .bind(project_ref_key(&request.project_id))
    .fetch_optional(&state.inner.pool)
    .await?;
    let Some(row) = row else {
        return Ok(DaemonProjectCheckout {
            project_id: request.project_id,
            commit_id: None,
            ref_etag: None,
            commit_created_at: None,
            org_selection_revision: 0,
            selected_org_resource_ids: Vec::new(),
            resources: Vec::new(),
            ready: false,
        });
    };

    let storage = super::project_storage::resolve_active(state, &request.project_id).await?;

    let commit_id: Option<String> = row.try_get("commit_id")?;
    let ref_etag: String = row.try_get("etag")?;
    let generation = storage.generation_path(commit_id.as_deref().unwrap_or(EMPTY_GENERATION));
    if verify_generation_ref(&generation, &request.project_id, commit_id.as_deref()).is_err() {
        return Ok(DaemonProjectCheckout {
            project_id: request.project_id,
            commit_id,
            ref_etag: Some(ref_etag),
            commit_created_at: None,
            org_selection_revision: 0,
            selected_org_resource_ids: Vec::new(),
            resources: Vec::new(),
            ready: false,
        });
    }

    let project_id = request.project_id;
    let checkout_commit_id = commit_id.clone();
    let mut checkout = tokio::task::spawn_blocking(move || {
        load_project_checkout(&generation, &project_id, checkout_commit_id.as_deref())
    })
    .await
    .map_err(|error| DaemonError::Server(format!("Project snapshot task failed: {error}")))??;
    checkout.ref_etag = Some(ref_etag);
    Ok(checkout)
}

async fn sync_refs(state: &DaemonState, project_ids: BTreeSet<String>) -> CommitSyncOutcome {
    let attempted_project_ids = project_ids.clone();
    let mut errors = BTreeMap::new();
    let access = match crate::project_access::require_current(state) {
        Ok(access) => access,
        Err(error) => {
            return CommitSyncOutcome::from_shared_error(attempted_project_ids, errors, error);
        }
    };
    let mut ready_project_ids = Vec::new();
    for project_id in project_ids {
        if !access.project_ids.contains(&project_id) {
            continue;
        }
        if let Err(error) = validate_cache_component("project_id", &project_id) {
            errors.insert(project_id, error);
            continue;
        }
        match ensure_active_draft_base_commits(state, &project_id).await {
            Ok(()) => ready_project_ids.push(project_id),
            Err(error) => {
                errors.insert(project_id, error);
            }
        }
    }

    let project_gate = std::sync::Arc::new(tokio::sync::Semaphore::new(
        MAX_CONCURRENT_PROJECT_STATE_REQUESTS,
    ));
    let mut project_tasks = tokio::task::JoinSet::new();
    for project_id in ready_project_ids {
        let state = state.clone();
        let project_gate = project_gate.clone();
        project_tasks.spawn(async move {
            let result = async {
                let _permit = project_gate.acquire_owned().await.map_err(|_| {
                    DaemonError::Server("Project Commit-state request gate closed".to_owned())
                })?;
                fetch_project_ref(&state, &project_id).await
            }
            .await;
            (project_id, result)
        });
    }

    let mut projects = BTreeMap::new();
    while let Some(result) = project_tasks.join_next().await {
        let (project_id, result) = match result {
            Ok(result) => result,
            Err(error) => {
                return CommitSyncOutcome::from_shared_error(
                    attempted_project_ids,
                    errors,
                    DaemonError::Server(format!(
                        "Project Commit-state request task failed: {error}"
                    )),
                );
            }
        };
        match result {
            Ok(project) => {
                projects.insert(project_id, project);
            }
            Err(error) => {
                errors.insert(project_id, error);
            }
        }
    }
    let expected_org_id = access.org_id.clone();
    let local_org_commit =
        match load_ref_commit(&state.inner.pool, &org_ref_key(&expected_org_id)).await {
            Ok(commit_id) => commit_id,
            Err(error) => {
                return CommitSyncOutcome::from_shared_error(attempted_project_ids, errors, error);
            }
        };
    let org_result = async {
        let (org_state, org_etag) = fetch_commit_state(
            state,
            "/api/v1/org/commit-state",
            local_org_commit.as_deref(),
        )
        .await?;
        validate_commit_state(
            &org_state,
            &org_etag,
            ServerCommitScope::Org,
            None,
            local_org_commit.as_deref(),
        )?;
        Ok::<_, DaemonError>((org_state, org_etag))
    }
    .await;
    let (org_state, org_etag) = match org_result {
        Ok(org) => org,
        Err(error) => {
            return CommitSyncOutcome::from_shared_error_after_project_errors(
                attempted_project_ids,
                errors,
                error,
            );
        }
    };

    if org_state.reference.org_id != expected_org_id {
        return CommitSyncOutcome::from_shared_error_after_project_errors(
            attempted_project_ids,
            errors,
            DaemonError::Server(
                "Project and organization commit states belong to different organizations"
                    .to_owned(),
            ),
        );
    }
    if crate::project_access::current(state).is_none() {
        return CommitSyncOutcome::from_shared_error(
            attempted_project_ids,
            errors,
            DaemonError::State {
                code: "sync_session_changed",
                message: "The Server session changed during synchronization.".to_owned(),
            },
        );
    }
    if let Err(error) = install_ref(state, &org_state, &org_etag, None).await {
        return CommitSyncOutcome::from_shared_error_after_project_errors(
            attempted_project_ids,
            errors,
            error,
        );
    }

    for (project_id, (project_state, project_etag)) in projects {
        let result = if project_state.reference.org_id != org_state.reference.org_id {
            Err(DaemonError::Server(
                "Project and organization commit states belong to different organizations"
                    .to_owned(),
            ))
        } else {
            install_ref(state, &project_state, &project_etag, Some(&project_id)).await
        };
        if let Err(error) = result {
            errors.insert(project_id, error);
        }
    }
    CommitSyncOutcome::from_project_errors(attempted_project_ids, errors)
}

pub(crate) async fn sync_project_ids(state: &DaemonState) -> Result<BTreeSet<String>, DaemonError> {
    let config = state.project_config();
    let server_url = canonical_server_url(&config.server_url)?;
    let mut project_ids = BTreeSet::new();
    if let Some(project_id) = config.project_id {
        project_ids.insert(project_id);
    }
    let bound_project_ids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT project_id
         FROM project_bindings
         WHERE server_url = $1
         ORDER BY project_id",
    )
    .bind(server_url)
    .fetch_all(&state.inner.pool)
    .await?;
    project_ids.extend(bound_project_ids);
    let draft_project_ids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT project_id
         FROM local_drafts
         WHERE status IN ('open', 'submitted')
         ORDER BY project_id",
    )
    .fetch_all(&state.inner.pool)
    .await?;
    project_ids.extend(draft_project_ids);
    Ok(project_ids)
}

async fn fetch_project_ref(
    state: &DaemonState,
    project_id: &str,
) -> Result<(ServerCommitState, String), DaemonError> {
    let local_project_commit =
        load_ref_commit(&state.inner.pool, &project_ref_key(project_id)).await?;
    let (project_state, project_etag) = fetch_commit_state(
        state,
        &format!("/api/v1/projects/{project_id}/commit-state"),
        local_project_commit.as_deref(),
    )
    .await?;
    validate_commit_state(
        &project_state,
        &project_etag,
        ServerCommitScope::Project,
        Some(project_id),
        local_project_commit.as_deref(),
    )?;

    Ok((project_state, project_etag))
}

async fn ensure_active_draft_base_commits(
    state: &DaemonState,
    project_id: &str,
) -> Result<(), DaemonError> {
    let commit_ids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT base_commit_id
         FROM local_drafts
         WHERE project_id = $1
           AND status IN ('open', 'submitted')
           AND base_commit_id IS NOT NULL
         ORDER BY base_commit_id",
    )
    .bind(project_id)
    .fetch_all(&state.inner.pool)
    .await?;
    for commit_id in commit_ids {
        ensure_commit_cached(state, &commit_id).await?;
    }
    Ok(())
}

async fn fetch_commit_state(
    state: &DaemonState,
    path: &str,
    local_commit_id: Option<&str>,
) -> Result<(ServerCommitState, String), DaemonError> {
    let path = match local_commit_id {
        Some(commit_id) => format!("{path}?local_commit_id={commit_id}"),
        None => path.to_owned(),
    };
    let response = execute_authenticated_server_request(
        state,
        reqwest::Method::GET,
        &path,
        &BTreeMap::new(),
        None,
    )
    .await?;
    let response = ensure_server_success(response).await?;
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .ok_or_else(|| DaemonError::Server("Commit state response is missing ETag".to_owned()))?;
    let commit_state = decode_server_json(response).await?;
    Ok((commit_state, etag))
}

fn validate_commit_state(
    state: &ServerCommitState,
    etag: &str,
    scope: ServerCommitScope,
    project_id: Option<&str>,
    local_commit_id: Option<&str>,
) -> Result<(), DaemonError> {
    if state.reference.name != MAIN_REF
        || state.reference.scope != scope
        || state.reference.project_id.as_deref() != project_id
    {
        return Err(DaemonError::Server(
            "Server returned a commit state for the wrong Ref".to_owned(),
        ));
    }
    let remote_commit_id = state.reference.commit_id.as_deref();
    if state.update_available != (local_commit_id != remote_commit_id) {
        return Err(DaemonError::Server(
            "Server commit update flag does not match the returned Ref".to_owned(),
        ));
    }
    let expected_etag = remote_commit_id.unwrap_or(EMPTY_GENERATION);
    if etag.trim().trim_matches('"') != expected_etag {
        return Err(DaemonError::Server(
            "Server commit state ETag does not match the returned Ref".to_owned(),
        ));
    }
    match (remote_commit_id, state.latest.as_ref()) {
        (Some(commit_id), Some(latest)) if latest.commit_id == commit_id => {}
        (None, None) => {}
        _ => {
            return Err(DaemonError::Server(
                "Server commit state latest Commit does not match the returned Ref".to_owned(),
            ));
        }
    }
    let expected_download = remote_commit_id.map(|id| format!("/api/v1/commits/{id}"));
    if state.download_url != expected_download {
        return Err(DaemonError::Server(
            "Server commit state download URL does not match the returned Ref".to_owned(),
        ));
    }
    Ok(())
}

async fn install_ref(
    state: &DaemonState,
    commit_state: &ServerCommitState,
    etag: &str,
    materialized_project_id: Option<&str>,
) -> Result<(), DaemonError> {
    let remote_commit_id = commit_state.reference.commit_id.as_deref();
    if let Some(commit_id) = remote_commit_id {
        validate_cache_component("commit_id", commit_id)?;
        let commit_cached = cached_commit_exists(&state.inner.pool, commit_id).await?;
        let generation_ready = match materialized_project_id {
            Some(project_id) => {
                let storage = super::project_storage::resolve_active(state, project_id).await?;
                let root = storage.generation_path(commit_id);
                if root.exists() {
                    verify_generation_ref(&root, project_id, Some(commit_id))?;
                    true
                } else {
                    false
                }
            }
            None => true,
        };
        if commit_cached && generation_ready {
            let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
            upsert_ref(&mut tx, &commit_state.reference, etag).await?;
            tx.commit().await?;
            state.inner.search_index_notify.notify_one();
            return Ok(());
        }

        let payload: ServerCommitPayload =
            get_server_json(state, &format!("/api/v1/commits/{commit_id}")).await?;
        validate_commit_payload(&payload, commit_state)?;
        let mut installed_location_revision = None;
        if let Some(project_id) = materialized_project_id {
            let storage = super::project_storage::resolve_active(state, project_id).await?;
            installed_location_revision = Some(storage.location_revision);
            let managed_root = storage.managed_root;
            let project_id = project_id.to_owned();
            let materialized_payload = payload.clone();
            tokio::task::spawn_blocking(move || {
                ensure_project_generation(&managed_root, &project_id, &materialized_payload)
            })
            .await
            .map_err(|error| {
                DaemonError::Server(format!("Commit materialization task failed: {error}"))
            })??;
        }

        let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let (Some(project_id), Some(expected_revision)) =
            (materialized_project_id, installed_location_revision)
        {
            let storage = super::project_storage::resolve_active(state, project_id).await?;
            if storage.location_revision != expected_revision {
                return Err(DaemonError::State {
                    code: "storage_move_conflict",
                    message:
                        "Project storage changed while a Commit generation was being installed"
                            .to_owned(),
                });
            }
        }
        cache_commit_payload(&mut tx, &payload).await?;
        upsert_ref(&mut tx, &commit_state.reference, etag).await?;
        tx.commit().await?;
        state.inner.search_index_notify.notify_one();
    } else {
        if let Some(project_id) = materialized_project_id {
            let storage = super::project_storage::resolve_active(state, project_id).await?;
            let managed_root = storage.managed_root;
            let project_id = project_id.to_owned();
            tokio::task::spawn_blocking(move || {
                ensure_empty_generation(&managed_root, &project_id)
            })
            .await
            .map_err(|error| {
                DaemonError::Server(format!("Commit materialization task failed: {error}"))
            })??;
        }
        let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
        upsert_ref(&mut tx, &commit_state.reference, etag).await?;
        tx.commit().await?;
        state.inner.search_index_notify.notify_one();
    }
    Ok(())
}

pub(super) async fn verify_current_project_generation_at(
    state: &DaemonState,
    project_id: &str,
    generations_root: &Path,
) -> Result<(), DaemonError> {
    let row = sqlx::query(
        "SELECT commit_id FROM cached_refs
         WHERE scope = 'project' AND project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(&state.inner.pool)
    .await?;
    let Some(row) = row else {
        return Ok(());
    };
    let commit_id: Option<String> = row.try_get("commit_id")?;
    let generation = generations_root.join(commit_id.as_deref().unwrap_or(EMPTY_GENERATION));
    verify_generation_ref(&generation, project_id, commit_id.as_deref())
}

fn validate_commit_payload(
    payload: &ServerCommitPayload,
    state: &ServerCommitState,
) -> Result<(), DaemonError> {
    let latest = state.latest.as_ref().ok_or_else(|| {
        DaemonError::Server("Commit payload was returned for an empty Ref".to_owned())
    })?;
    if &payload.commit != latest
        || payload.commit.tree_id != payload.tree.tree_id
        || payload.commit.scope != state.reference.scope
        || payload.commit.org_id != state.reference.org_id
        || payload.commit.project_id != state.reference.project_id
        || payload.commit.version < 1
        || payload.commit.parent_commit_id.as_deref() == Some(payload.commit.commit_id.as_str())
    {
        return Err(DaemonError::Server(
            "Server returned an inconsistent Commit payload".to_owned(),
        ));
    }

    let mut blobs = HashMap::new();
    for blob in &payload.blobs {
        if blob_object_id(&blob.content) != blob.blob_id {
            return Err(DaemonError::Server(format!(
                "Blob {} failed content-address verification",
                blob.blob_id
            )));
        }
        if blobs.insert(blob.blob_id.as_str(), blob).is_some() {
            return Err(DaemonError::Server(format!(
                "Commit payload contains duplicate Blob {}",
                blob.blob_id
            )));
        }
    }

    let mut entry_ids = BTreeSet::new();
    let mut referenced_blobs = BTreeSet::new();
    let mut selection_blob_id = None;
    for entry in &payload.tree.entries {
        if !entry_ids.insert(entry.id.as_str()) {
            return Err(DaemonError::Server(format!(
                "Tree contains duplicate entry {}",
                entry.id
            )));
        }
        if !blobs.contains_key(entry.blob_id.as_str()) {
            return Err(DaemonError::Server(format!(
                "Tree entry {} references a missing Blob",
                entry.id
            )));
        }
        referenced_blobs.insert(entry.blob_id.as_str());
        validate_tree_entry_ownership(entry, &payload.commit)?;
        if entry.kind == ServerTreeEntryKind::ProjectOrgSelection {
            if selection_blob_id.replace(entry.blob_id.as_str()).is_some() {
                return Err(DaemonError::Server(
                    "Project Commit contains more than one organization selection".to_owned(),
                ));
            }
        } else {
            materialized_resource_content(entry.kind, &blobs[entry.blob_id.as_str()].content)?;
        }
    }
    validate_materialization_paths(&payload.tree.entries)?;
    if referenced_blobs.len() != blobs.len() {
        return Err(DaemonError::Server(
            "Commit payload contains unreferenced Blobs".to_owned(),
        ));
    }

    match payload.commit.scope {
        ServerCommitScope::Project => {
            let project_id = payload.commit.project_id.as_deref().ok_or_else(|| {
                DaemonError::Server("Project Commit is missing project_id".to_owned())
            })?;
            let selection = payload.project_org_selection.as_ref().ok_or_else(|| {
                DaemonError::Server(
                    "Project Commit is missing its organization selection".to_owned(),
                )
            })?;
            let selection_blob_id = selection_blob_id.ok_or_else(|| {
                DaemonError::Server(
                    "Project Commit is missing its organization selection".to_owned(),
                )
            })?;
            let selection_blob = blobs
                .get(selection_blob_id)
                .expect("selection Blob existence was validated");
            let selection_from_blob: serde_json::Value =
                serde_json::from_str(&selection_blob.content).map_err(|error| {
                    DaemonError::Server(format!(
                        "Project organization selection Blob is invalid: {error}"
                    ))
                })?;
            if &selection_from_blob != selection {
                return Err(DaemonError::Server(
                    "Project organization selection does not match its Blob".to_owned(),
                ));
            }
            if selection
                .get("project_id")
                .and_then(serde_json::Value::as_str)
                != Some(project_id)
            {
                return Err(DaemonError::Server(
                    "Project Commit organization selection belongs to another project".to_owned(),
                ));
            }
        }
        ServerCommitScope::Org => {
            if selection_blob_id.is_some() || payload.project_org_selection.is_some() {
                return Err(DaemonError::Server(
                    "Organization Commit contains a project organization selection".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_tree_entry_ownership(
    entry: &ServerTreeEntry,
    commit: &ServerCommit,
) -> Result<(), DaemonError> {
    let valid = match (commit.scope, entry.kind, entry.scope) {
        (
            ServerCommitScope::Project,
            ServerTreeEntryKind::ProjectOrgSelection,
            ServerTreeEntryScope::Daemon,
        ) => {
            entry.project_id == commit.project_id
                && entry.path.is_none()
                && entry.source == ServerTreeEntrySource::Config
        }
        (ServerCommitScope::Project, _, ServerTreeEntryScope::Project) => {
            entry.project_id == commit.project_id && entry.source == ServerTreeEntrySource::Project
        }
        (ServerCommitScope::Project, _, ServerTreeEntryScope::Org) => {
            entry.project_id.is_none() && entry.source == ServerTreeEntrySource::SelectedOrg
        }
        (ServerCommitScope::Org, _, ServerTreeEntryScope::Org) => {
            entry.project_id.is_none() && entry.source == ServerTreeEntrySource::Org
        }
        _ => false,
    };
    if !valid {
        return Err(DaemonError::Server(format!(
            "Tree entry {} has an invalid scope, source, or project ownership",
            entry.id
        )));
    }
    Ok(())
}

fn validate_resource_path(entry: &ServerTreeEntry) -> Result<(), DaemonError> {
    let path = entry
        .path
        .as_deref()
        .ok_or_else(|| DaemonError::Server(format!("Tree entry {} is missing a path", entry.id)))?;
    validate_relative_path(path)?;
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), DaemonError> {
    if !is_normalized_relative_path(value) {
        return Err(DaemonError::Server(format!(
            "Tree path is not a portable normalized relative path: {value}"
        )));
    }
    Ok(())
}

fn validate_materialization_paths(entries: &[ServerTreeEntry]) -> Result<(), DaemonError> {
    let mut paths = BTreeMap::<String, (String, String)>::new();
    for entry in entries {
        if entry.kind == ServerTreeEntryKind::ProjectOrgSelection {
            continue;
        }
        validate_resource_path(entry)?;
        let output_path = materialization_output_path(entry)?;
        insert_materialization_path(&mut paths, &entry.id, &output_path)?;
    }
    Ok(())
}

fn materialization_output_path(entry: &ServerTreeEntry) -> Result<String, DaemonError> {
    let path = entry
        .path
        .as_deref()
        .ok_or_else(|| DaemonError::Server(format!("Tree entry {} is missing a path", entry.id)))?;
    match entry.kind {
        ServerTreeEntryKind::Rule
        | ServerTreeEntryKind::Context
        | ServerTreeEntryKind::Workflow
        | ServerTreeEntryKind::Memory => Ok(format!("cache/memory/{path}")),
        ServerTreeEntryKind::ProjectOrgSelection => Err(DaemonError::Server(
            "organization selection does not materialize as a file".to_owned(),
        )),
    }
}

fn insert_materialization_path(
    paths: &mut BTreeMap<String, (String, String)>,
    entry_id: &str,
    output_path: &str,
) -> Result<(), DaemonError> {
    let normalized = output_path.to_lowercase();
    if let Some((existing_id, existing_path)) = paths.get(&normalized) {
        return Err(DaemonError::Server(format!(
            "Tree materializes {existing_id} at {existing_path} and {entry_id} at {output_path}, which conflict"
        )));
    }
    for (index, _) in normalized.rmatch_indices('/') {
        if let Some((existing_id, existing_path)) = paths.get(&normalized[..index]) {
            return Err(DaemonError::Server(format!(
                "Tree materializes {existing_id} at {existing_path} and {entry_id} at {output_path}, which conflict"
            )));
        }
    }
    let descendant_prefix = format!("{normalized}/");
    if let Some((_, (existing_id, existing_path))) = paths
        .range(descendant_prefix.clone()..)
        .next()
        .filter(|(path, _)| path.starts_with(&descendant_prefix))
    {
        return Err(DaemonError::Server(format!(
            "Tree materializes {existing_id} at {existing_path} and {entry_id} at {output_path}, which conflict"
        )));
    }
    paths.insert(normalized, (entry_id.to_owned(), output_path.to_owned()));
    Ok(())
}

async fn cache_commit_payload(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    payload: &ServerCommitPayload,
) -> Result<(), DaemonError> {
    for blob in &payload.blobs {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT content FROM cached_blobs WHERE blob_id = $1")
                .bind(&blob.blob_id)
                .fetch_optional(&mut **tx)
                .await?;
        match existing {
            Some(content) if content != blob.content => {
                return Err(DaemonError::Server(format!(
                    "Cached Blob {} violates immutability",
                    blob.blob_id
                )));
            }
            Some(_) => {}
            None => {
                sqlx::query("INSERT INTO cached_blobs (blob_id, content) VALUES ($1, $2)")
                    .bind(&blob.blob_id)
                    .bind(&blob.content)
                    .execute(&mut **tx)
                    .await?;
            }
        }
    }

    let tree_json = serde_json::to_string(&payload.tree)?;
    let existing_tree: Option<String> =
        sqlx::query_scalar("SELECT payload_json FROM cached_trees WHERE tree_id = $1")
            .bind(&payload.tree.tree_id)
            .fetch_optional(&mut **tx)
            .await?;
    match existing_tree {
        Some(value) if value != tree_json => {
            return Err(DaemonError::Server(format!(
                "Cached Tree {} violates immutability",
                payload.tree.tree_id
            )));
        }
        Some(_) => {}
        None => {
            sqlx::query("INSERT INTO cached_trees (tree_id, payload_json) VALUES ($1, $2)")
                .bind(&payload.tree.tree_id)
                .bind(&tree_json)
                .execute(&mut **tx)
                .await?;
        }
    }
    let commit_json = serde_json::to_string(&payload.commit)?;
    let existing: Option<String> =
        sqlx::query_scalar("SELECT payload_json FROM cached_commits WHERE commit_id = $1")
            .bind(&payload.commit.commit_id)
            .fetch_optional(&mut **tx)
            .await?;
    match existing {
        Some(value) if value != commit_json => {
            return Err(DaemonError::Server(format!(
                "Cached Commit {} violates immutability",
                payload.commit.commit_id
            )));
        }
        Some(_) => {}
        None => {
            sqlx::query(
                "INSERT INTO cached_commits (
                    commit_id, scope, org_id, project_id, tree_id,
                    parent_commit_id, version, created_at, payload_json
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(&payload.commit.commit_id)
            .bind(payload.commit.scope.as_str())
            .bind(&payload.commit.org_id)
            .bind(&payload.commit.project_id)
            .bind(&payload.commit.tree_id)
            .bind(&payload.commit.parent_commit_id)
            .bind(payload.commit.version)
            .bind(&payload.commit.created_at)
            .bind(commit_json)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

async fn upsert_ref(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    reference: &ServerRef,
    etag: &str,
) -> Result<(), DaemonError> {
    let key = match reference.scope {
        ServerCommitScope::Org => org_ref_key(&reference.org_id),
        ServerCommitScope::Project => {
            project_ref_key(reference.project_id.as_deref().ok_or_else(|| {
                DaemonError::Server("Project Ref is missing project_id".to_owned())
            })?)
        }
    };
    let previous_commit_id: Option<Option<String>> =
        sqlx::query_scalar("SELECT commit_id FROM cached_refs WHERE ref_key = $1")
            .bind(&key)
            .fetch_optional(&mut **tx)
            .await?;
    let ref_changed = previous_commit_id.as_ref() != Some(&reference.commit_id);
    sqlx::query(
        "INSERT INTO cached_refs (
            ref_key, name, scope, org_id, project_id, commit_id, etag, server_updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT(ref_key) DO UPDATE SET
            name = excluded.name,
            scope = excluded.scope,
            org_id = excluded.org_id,
            project_id = excluded.project_id,
            commit_id = excluded.commit_id,
            etag = excluded.etag,
            server_updated_at = excluded.server_updated_at,
            installed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(key)
    .bind(&reference.name)
    .bind(reference.scope.as_str())
    .bind(&reference.org_id)
    .bind(&reference.project_id)
    .bind(&reference.commit_id)
    .bind(etag)
    .bind(&reference.updated_at)
    .execute(&mut **tx)
    .await?;
    match reference.scope {
        ServerCommitScope::Project => {
            let project_id = reference.project_id.as_deref().ok_or_else(|| {
                DaemonError::Server("Project Ref is missing project_id".to_owned())
            })?;
            sqlx::query(
                "UPDATE local_drafts
                 SET current_commit_id = $2,
                     freshness = CASE WHEN base_commit_id IS $2 THEN 'current' ELSE 'behind' END,
                     reconciliation = CASE WHEN current_commit_id IS $2 THEN reconciliation ELSE 'unknown' END,
                     reconciliation_candidate_id = CASE WHEN current_commit_id IS $2 THEN reconciliation_candidate_id ELSE NULL END
                 WHERE project_id = $1 AND resource_scope = 'project'",
            )
            .bind(project_id)
            .bind(&reference.commit_id)
            .execute(&mut **tx)
            .await?;
            refresh_upstream_resource_changes(tx, "project", project_id).await?;
        }
        ServerCommitScope::Org => {
            sqlx::query(
                "UPDATE local_drafts
                 SET current_commit_id = $2,
                     freshness = CASE WHEN base_commit_id IS $2 THEN 'current' ELSE 'behind' END,
                     reconciliation = CASE WHEN current_commit_id IS $2 THEN reconciliation ELSE 'unknown' END,
                     reconciliation_candidate_id = CASE WHEN current_commit_id IS $2 THEN reconciliation_candidate_id ELSE NULL END
                 WHERE resource_scope = 'org'
                   AND project_id IN (
                       SELECT project_id FROM cached_refs
                       WHERE scope = 'project' AND org_id = $1 AND project_id IS NOT NULL
                   )",
            )
            .bind(&reference.org_id)
            .bind(&reference.commit_id)
            .execute(&mut **tx)
            .await?;
            refresh_upstream_resource_changes(tx, "org", &reference.org_id).await?;
        }
    }
    if ref_changed {
        match reference.scope {
            ServerCommitScope::Project => {
                let project_id = reference.project_id.as_deref().ok_or_else(|| {
                    DaemonError::Server("Project Ref is missing project_id".to_owned())
                })?;
                search::scheduler::enqueue_project_in_tx(tx, project_id).await?;
            }
            ServerCommitScope::Org => {
                search::scheduler::enqueue_all_cached_projects_in_tx(tx).await?;
            }
        }
    }
    Ok(())
}

async fn refresh_upstream_resource_changes(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    scope: &str,
    owner_id: &str,
) -> Result<(), DaemonError> {
    let rows = match scope {
        "project" => {
            sqlx::query(
                "SELECT draft_id, base_commit_id, current_commit_id, target_id, resource_kind
                 FROM local_drafts
                 WHERE resource_scope = 'project' AND project_id = $1",
            )
            .bind(owner_id)
            .fetch_all(&mut **tx)
            .await?
        }
        "org" => {
            sqlx::query(
                "SELECT d.draft_id, d.base_commit_id, d.current_commit_id, d.target_id,
                        d.resource_kind
                 FROM local_drafts d
                 WHERE d.resource_scope = 'org'
                   AND d.project_id IN (
                       SELECT project_id FROM cached_refs
                       WHERE scope = 'project' AND org_id = $1 AND project_id IS NOT NULL
                   )",
            )
            .bind(owner_id)
            .fetch_all(&mut **tx)
            .await?
        }
        _ => {
            return Err(DaemonError::Server(format!(
                "cannot refresh Draft resource changes for unknown scope {scope}"
            )));
        }
    };

    for row in rows {
        let draft_id: String = row.try_get("draft_id")?;
        let base_commit_id: Option<String> = row.try_get("base_commit_id")?;
        let current_commit_id: Option<String> = row.try_get("current_commit_id")?;
        let target_id: Option<String> = row.try_get("target_id")?;
        let resource_kind: String = row.try_get("resource_kind")?;
        let changed = if base_commit_id == current_commit_id {
            false
        } else if let Some(target_id) = target_id.as_deref() {
            cached_resource_entry(tx, base_commit_id.as_deref(), target_id, &resource_kind).await?
                != cached_resource_entry(
                    tx,
                    current_commit_id.as_deref(),
                    target_id,
                    &resource_kind,
                )
                .await?
        } else {
            false
        };
        sqlx::query(
            "UPDATE local_drafts
             SET has_upstream_resource_changes = $2
             WHERE draft_id = $1",
        )
        .bind(draft_id)
        .bind(changed)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn cached_resource_entry(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    commit_id: Option<&str>,
    target_id: &str,
    resource_kind: &str,
) -> Result<Option<(Option<String>, String)>, DaemonError> {
    let Some(commit_id) = commit_id else {
        return Ok(None);
    };
    let tree_json: String = sqlx::query_scalar(
        "SELECT t.payload_json
         FROM cached_commits c
         JOIN cached_trees t ON t.tree_id = c.tree_id
         WHERE c.commit_id = $1",
    )
    .bind(commit_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        DaemonError::Server(format!(
            "cached Commit {commit_id} is unavailable while refreshing Draft state"
        ))
    })?;
    let tree: ServerTree = serde_json::from_str(&tree_json)?;
    let expected_kind = match resource_kind {
        "memory" => ServerTreeEntryKind::Memory,
        "context" => ServerTreeEntryKind::Context,
        "rule" => ServerTreeEntryKind::Rule,
        "workflow" => ServerTreeEntryKind::Workflow,
        _ => {
            return Err(DaemonError::Server(format!(
                "unknown Draft resource kind {resource_kind}"
            )));
        }
    };
    Ok(tree
        .entries
        .into_iter()
        .find(|entry| entry.id == target_id && entry.kind == expected_kind)
        .map(|entry| (entry.path, entry.blob_id)))
}

async fn load_ref_commit(pool: &SqlitePool, key: &str) -> Result<Option<String>, DaemonError> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT commit_id FROM cached_refs WHERE ref_key = $1",
    )
    .bind(key)
    .fetch_optional(pool)
    .await?
    .flatten())
}

async fn cached_commit_exists(pool: &SqlitePool, commit_id: &str) -> Result<bool, DaemonError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cached_commits WHERE commit_id = $1")
        .bind(commit_id)
        .fetch_one(pool)
        .await?;
    Ok(count == 1)
}

fn ensure_project_generation(
    managed_root: &Path,
    project_id: &str,
    payload: &ServerCommitPayload,
) -> Result<PathBuf, DaemonError> {
    let marker = serde_json::to_vec(payload)?;
    ensure_generation(
        managed_root,
        project_id,
        &payload.commit.commit_id,
        &marker,
        |root| materialize_payload(root, project_id, payload),
    )
}

fn ensure_empty_generation(managed_root: &Path, project_id: &str) -> Result<PathBuf, DaemonError> {
    let marker = serde_json::to_vec(&json!({
        "project_id": project_id,
        "commit_id": null
    }))?;
    ensure_generation(
        managed_root,
        project_id,
        EMPTY_GENERATION,
        &marker,
        |root| {
            std::fs::create_dir_all(root.join("cache/memory"))?;
            let manifest = MaterializedManifest {
                project_id,
                commit_id: None,
                tree_id: None,
                ref_name: MAIN_REF,
                memories: BTreeMap::new(),
            };
            std::fs::write(
                root.join("manifest.json"),
                serde_json::to_vec_pretty(&manifest)?,
            )?;
            Ok(())
        },
    )
}

fn ensure_generation(
    managed_root: &Path,
    project_id: &str,
    generation: &str,
    marker: &[u8],
    materialize: impl FnOnce(&Path) -> Result<(), DaemonError>,
) -> Result<PathBuf, DaemonError> {
    validate_cache_component("project_id", project_id)?;
    validate_cache_component("generation", generation)?;
    let generations = managed_root.join("generations");
    super::project_storage::ensure_private_directory(&generations)?;
    let final_root = generations.join(generation);
    if final_root.exists() {
        verify_generation_marker(&final_root, marker)?;
        return Ok(final_root);
    }

    let temporary_root = generations.join(format!(".tmp-{}", Uuid::new_v4().simple()));
    std::fs::create_dir(&temporary_root)?;
    let build_result = (|| {
        materialize(&temporary_root)?;
        super::project_storage::write_private_file(
            &temporary_root.join("commit-payload.json"),
            marker,
        )?;
        super::project_storage::secure_managed_tree(&temporary_root)?;
        Ok::<(), DaemonError>(())
    })();
    if let Err(error) = build_result {
        let _ = std::fs::remove_dir_all(&temporary_root);
        return Err(error);
    }

    match std::fs::rename(&temporary_root, &final_root) {
        Ok(()) => Ok(final_root),
        Err(_error) if final_root.exists() => {
            let _ = std::fs::remove_dir_all(&temporary_root);
            verify_generation_marker(&final_root, marker)?;
            Ok(final_root)
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temporary_root);
            Err(error.into())
        }
    }
}

fn verify_generation_marker(root: &Path, expected: &[u8]) -> Result<(), DaemonError> {
    let actual = std::fs::read(root.join("commit-payload.json"))?;
    if actual != expected {
        return Err(DaemonError::Server(format!(
            "Materialized generation {} violates Commit immutability",
            root.display()
        )));
    }
    Ok(())
}

fn verify_generation_ref(
    root: &Path,
    project_id: &str,
    commit_id: Option<&str>,
) -> Result<(), DaemonError> {
    if !root.is_dir() {
        return Err(DaemonError::Server(format!(
            "Materialized generation {} is missing",
            root.display()
        )));
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json"))?)?;
    let manifest_commit_matches = match commit_id {
        Some(commit_id) => {
            manifest.get("commit_id").and_then(|value| value.as_str()) == Some(commit_id)
        }
        None => manifest
            .get("commit_id")
            .is_some_and(serde_json::Value::is_null),
    };
    if manifest.get("project_id").and_then(|value| value.as_str()) != Some(project_id)
        || !manifest_commit_matches
        || manifest.get("ref_name").and_then(|value| value.as_str()) != Some(MAIN_REF)
    {
        return Err(DaemonError::Server(format!(
            "Materialized generation {} does not match its Ref",
            root.display()
        )));
    }

    let marker = std::fs::read(root.join("commit-payload.json"))?;
    match commit_id {
        Some(commit_id) => {
            let payload: ServerCommitPayload = serde_json::from_slice(&marker)?;
            if payload.commit.commit_id != commit_id
                || payload.commit.scope != ServerCommitScope::Project
                || payload.commit.project_id.as_deref() != Some(project_id)
                || payload.commit.tree_id != payload.tree.tree_id
            {
                return Err(DaemonError::Server(format!(
                    "Materialized generation {} has an invalid Commit marker",
                    root.display()
                )));
            }
        }
        None => {
            let marker: serde_json::Value = serde_json::from_slice(&marker)?;
            if marker.get("project_id").and_then(|value| value.as_str()) != Some(project_id)
                || !marker
                    .get("commit_id")
                    .is_some_and(serde_json::Value::is_null)
            {
                return Err(DaemonError::Server(format!(
                    "Materialized generation {} has an invalid empty Ref marker",
                    root.display()
                )));
            }
        }
    }
    Ok(())
}

fn load_project_checkout(
    root: &Path,
    project_id: &str,
    commit_id: Option<&str>,
) -> Result<DaemonProjectCheckout, DaemonError> {
    verify_generation_ref(root, project_id, commit_id)?;
    let Some(commit_id) = commit_id else {
        return Ok(DaemonProjectCheckout {
            project_id: project_id.to_owned(),
            commit_id: None,
            ref_etag: None,
            commit_created_at: None,
            org_selection_revision: 0,
            selected_org_resource_ids: Vec::new(),
            resources: Vec::new(),
            ready: true,
        });
    };

    let payload: ServerCommitPayload =
        serde_json::from_slice(&std::fs::read(root.join("commit-payload.json"))?)?;
    let blobs = payload
        .blobs
        .iter()
        .map(|blob| (blob.blob_id.as_str(), blob))
        .collect::<HashMap<_, _>>();
    let selection = payload.project_org_selection.as_ref().ok_or_else(|| {
        DaemonError::Server(
            "Cached Project Commit is missing its organization selection".to_owned(),
        )
    })?;
    if selection
        .get("project_id")
        .and_then(serde_json::Value::as_str)
        != Some(project_id)
    {
        return Err(DaemonError::Server(
            "Cached Project Commit organization selection belongs to another project".to_owned(),
        ));
    }
    let org_selection_revision = selection
        .get("revision")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| {
            DaemonError::Server(
                "Cached Project Commit organization selection is missing revision".to_owned(),
            )
        })?;

    let mut selected_org_resource_ids = BTreeSet::new();
    let mut resources = Vec::new();
    for entry in &payload.tree.entries {
        let scope = match entry.scope {
            ServerTreeEntryScope::Org => {
                selected_org_resource_ids.insert(entry.id.clone());
                DaemonDraftScope::Org
            }
            ServerTreeEntryScope::Project => DaemonDraftScope::Project,
            ServerTreeEntryScope::Daemon => continue,
        };
        let resource_kind = match entry.kind {
            ServerTreeEntryKind::Rule
            | ServerTreeEntryKind::Context
            | ServerTreeEntryKind::Workflow
            | ServerTreeEntryKind::Memory => DaemonDraftResourceKind::Memory,
            ServerTreeEntryKind::ProjectOrgSelection => continue,
        };
        let path = entry.path.clone().ok_or_else(|| {
            DaemonError::Server(format!("Tree entry {} is missing a path", entry.id))
        })?;
        let blob = blobs.get(entry.blob_id.as_str()).ok_or_else(|| {
            DaemonError::Server(format!("Tree entry {} references a missing Blob", entry.id))
        })?;
        resources.push(DaemonProjectCheckoutResource {
            resource_id: entry.id.clone(),
            scope,
            resource_kind,
            project_id: entry.project_id.clone(),
            path,
            content_hash: content_hash(&blob.content),
            content: project_checkout_content(entry.kind, &blob.content)?,
        });
    }

    Ok(DaemonProjectCheckout {
        project_id: project_id.to_owned(),
        commit_id: Some(commit_id.to_owned()),
        ref_etag: None,
        commit_created_at: Some(payload.commit.created_at),
        org_selection_revision,
        selected_org_resource_ids: selected_org_resource_ids.into_iter().collect(),
        resources,
        ready: true,
    })
}

fn project_checkout_content(
    kind: ServerTreeEntryKind,
    blob: &str,
) -> Result<DaemonDraftContent, DaemonError> {
    match kind {
        ServerTreeEntryKind::Context
        | ServerTreeEntryKind::Rule
        | ServerTreeEntryKind::Workflow
        | ServerTreeEntryKind::Memory => Ok(DaemonDraftContent {
            description: None,
            content: blob.to_owned(),
        }),
        ServerTreeEntryKind::ProjectOrgSelection => Err(DaemonError::Server(
            "Project organization selection is not a memory resource".to_owned(),
        )),
    }
}

fn materialize_payload(
    root: &Path,
    project_id: &str,
    payload: &ServerCommitPayload,
) -> Result<(), DaemonError> {
    validate_materialization_paths(&payload.tree.entries)?;
    let blobs = payload
        .blobs
        .iter()
        .map(|blob| (blob.blob_id.as_str(), blob))
        .collect::<HashMap<_, _>>();
    let mut memories = BTreeMap::new();

    for entry in &payload.tree.entries {
        if entry.kind == ServerTreeEntryKind::ProjectOrgSelection {
            continue;
        }
        let path = entry.path.as_deref().ok_or_else(|| {
            DaemonError::Server(format!("Tree entry {} is missing a path", entry.id))
        })?;
        let blob = blobs.get(entry.blob_id.as_str()).ok_or_else(|| {
            DaemonError::Server(format!("Tree entry {} references a missing Blob", entry.id))
        })?;
        let materialized_content = materialized_resource_content(entry.kind, &blob.content)?;
        let manifest_entry = MaterializedManifestEntry {
            path,
            hash: content_hash(&materialized_content),
            description: "",
        };
        memories.insert(entry.id.clone(), manifest_entry);
        let relative_output = PathBuf::from(materialization_output_path(entry)?);
        let output = root.join(relative_output);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(output, materialized_content.as_bytes())?;
    }

    std::fs::create_dir_all(root.join("cache/memory"))?;
    let manifest = MaterializedManifest {
        project_id,
        commit_id: Some(&payload.commit.commit_id),
        tree_id: Some(&payload.tree.tree_id),
        ref_name: MAIN_REF,
        memories,
    };
    std::fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}

fn materialized_resource_content(
    kind: ServerTreeEntryKind,
    blob: &str,
) -> Result<String, DaemonError> {
    match kind {
        ServerTreeEntryKind::Context
        | ServerTreeEntryKind::Rule
        | ServerTreeEntryKind::Workflow
        | ServerTreeEntryKind::Memory => Ok(blob.to_owned()),
        ServerTreeEntryKind::ProjectOrgSelection => Err(DaemonError::Server(
            "Project organization selection cannot be materialized as memory".to_owned(),
        )),
    }
}

pub(super) fn validate_cache_component(label: &str, value: &str) -> Result<(), DaemonError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        || value == "."
        || value == ".."
    {
        return Err(DaemonError::InvalidRequest(format!(
            "{label} is not a safe cache path component"
        )));
    }
    Ok(())
}

fn project_ref_key(project_id: &str) -> String {
    format!("project:{project_id}")
}

fn org_ref_key(org_id: &str) -> String {
    format!("org:{org_id}")
}

fn blob_object_id(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"blob");
    hasher.update([0]);
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ServerCommitState {
    update_available: bool,
    #[serde(rename = "ref")]
    reference: ServerRef,
    latest: Option<ServerCommit>,
    download_url: Option<String>,
    incremental_supported: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ServerRef {
    name: String,
    scope: ServerCommitScope,
    org_id: String,
    project_id: Option<String>,
    commit_id: Option<String>,
    updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ServerCommit {
    commit_id: String,
    scope: ServerCommitScope,
    org_id: String,
    project_id: Option<String>,
    tree_id: String,
    parent_commit_id: Option<String>,
    version: i64,
    created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ServerCommitPayload {
    commit: ServerCommit,
    tree: ServerTree,
    blobs: Vec<ServerBlob>,
    project_org_selection: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ServerTree {
    tree_id: String,
    entries: Vec<ServerTreeEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ServerTreeEntry {
    id: String,
    #[serde(rename = "type")]
    kind: ServerTreeEntryKind,
    scope: ServerTreeEntryScope,
    project_id: Option<String>,
    path: Option<String>,
    blob_id: String,
    source: ServerTreeEntrySource,
    #[serde(default)]
    description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ServerBlob {
    blob_id: String,
    content: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ServerCommitScope {
    Org,
    Project,
}

impl ServerCommitScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Project => "project",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ServerTreeEntryKind {
    Rule,
    Context,
    Workflow,
    Memory,
    ProjectOrgSelection,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ServerTreeEntryScope {
    Org,
    Project,
    Daemon,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ServerTreeEntrySource {
    Org,
    Project,
    SelectedOrg,
    Bootstrap,
    Config,
}

#[derive(Serialize)]
struct MaterializedManifest<'a> {
    project_id: &'a str,
    commit_id: Option<&'a str>,
    tree_id: Option<&'a str>,
    ref_name: &'a str,
    memories: BTreeMap<String, MaterializedManifestEntry<'a>>,
}

#[derive(Serialize)]
struct MaterializedManifestEntry<'a> {
    path: &'a str,
    hash: String,
    description: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_sync_running_scope_is_read_as_one_snapshot() {
        let running = std::sync::RwLock::new(CommitSyncRunScope::Project("prj_a".to_owned()));
        let snapshot = running.read().unwrap().clone();
        *running.write().unwrap() = CommitSyncRunScope::Idle;

        assert!(snapshot.is_running_for(None));
        assert!(snapshot.is_running_for(Some("prj_a")));
        assert!(!snapshot.is_running_for(Some("prj_b")));
        assert!(!running.read().unwrap().is_running_for(Some("prj_a")));
        assert!(CommitSyncRunScope::Global.is_running_for(Some("prj_b")));
    }

    #[test]
    fn shared_infrastructure_error_remains_the_primary_error() {
        let outcome = CommitSyncOutcome::from_shared_error(
            BTreeSet::from(["prj_a".to_owned(), "prj_b".to_owned()]),
            BTreeMap::from([(
                "prj_a".to_owned(),
                DaemonError::Server("project A failed".to_owned()),
            )]),
            DaemonError::Server("shared infrastructure failed".to_owned()),
        );

        assert_eq!(
            outcome.error.unwrap().to_string(),
            "server sync error: shared infrastructure failed"
        );
        assert!(outcome.project_errors["prj_a"].contains("project A failed"));
        assert!(outcome.project_errors["prj_b"].contains("shared infrastructure failed"));
    }

    #[test]
    fn content_address_verification_matches_server_blob_ids() {
        assert_eq!(
            blob_object_id("hello"),
            "b7da690ebce9312567657893e936fe6935d0a52372f2703f02616bbd53eccb0c"
        );
        assert_eq!(
            content_hash("hello"),
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn rejects_unsafe_cache_components_and_resource_paths() {
        assert!(validate_cache_component("project_id", "project_123").is_ok());
        assert!(validate_cache_component("project_id", "../project").is_err());
        assert!(validate_relative_path("spec/API.md").is_ok());
        assert!(validate_relative_path("../secrets").is_err());
        assert!(validate_relative_path("/absolute").is_err());
        assert!(validate_relative_path("spec//API.md").is_err());
        assert!(validate_relative_path("spec/AUX.md").is_err());
        assert!(validate_relative_path("spec/API.md ").is_err());
    }

    #[test]
    fn rejects_case_and_file_directory_materialization_collisions() {
        fn context_entry(id: &str, path: &str) -> ServerTreeEntry {
            ServerTreeEntry {
                id: id.to_owned(),
                kind: ServerTreeEntryKind::Memory,
                scope: ServerTreeEntryScope::Project,
                project_id: Some("prj_test".to_owned()),
                description: String::new(),
                path: Some(path.to_owned()),
                blob_id: format!("blob_{id}"),
                source: ServerTreeEntrySource::Project,
            }
        }

        assert!(
            validate_materialization_paths(&[
                context_entry("one", "spec/API.md"),
                context_entry("two", "spec/api.md"),
            ])
            .is_err()
        );
        assert!(
            validate_materialization_paths(&[
                context_entry("one", "spec/API.md"),
                context_entry("two", "spec/API.md/examples.md"),
            ])
            .is_err()
        );
        assert!(
            validate_materialization_paths(&[
                context_entry("one", "spec/API.md/examples.md"),
                context_entry("two", "spec/API.md"),
            ])
            .is_err()
        );
        assert!(
            validate_materialization_paths(&[
                context_entry("one", "spec/API.md"),
                context_entry("two", "spec/CLI.md"),
            ])
            .is_ok()
        );
    }

    #[test]
    fn rejects_materializing_the_system_selection_as_memory() {
        let error = materialized_resource_content(ServerTreeEntryKind::ProjectOrgSelection, "{}")
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Project organization selection cannot be materialized as memory")
        );
    }

    #[test]
    fn materializes_all_memory_resource_kinds() {
        let root = tempfile::tempdir().unwrap();
        let entries = [
            (
                "ctx_test",
                ServerTreeEntryKind::Context,
                ServerTreeEntryScope::Project,
                Some("prj_test"),
                "spec/API.md",
                ServerTreeEntrySource::Project,
                "Context body",
            ),
            (
                "rule_test",
                ServerTreeEntryKind::Rule,
                ServerTreeEntryScope::Org,
                None,
                "coding/STYLE.md",
                ServerTreeEntrySource::SelectedOrg,
                "# Style\n\nApply while coding.\n\nRule body",
            ),
            (
                "workflow_test",
                ServerTreeEntryKind::Workflow,
                ServerTreeEntryScope::Project,
                Some("prj_test"),
                "workflow/CODING.md",
                ServerTreeEntrySource::Project,
                "# Coding\n\nWorkflow body\n\n1. Run tests",
            ),
        ];
        let blobs = entries
            .iter()
            .map(|entry| ServerBlob {
                blob_id: blob_object_id(entry.6),
                content: entry.6.to_owned(),
            })
            .collect::<Vec<_>>();
        let tree_entries = entries
            .iter()
            .zip(&blobs)
            .map(|(entry, blob)| ServerTreeEntry {
                id: entry.0.to_owned(),
                kind: entry.1,
                scope: entry.2,
                project_id: entry.3.map(ToOwned::to_owned),
                description: String::new(),
                path: Some(entry.4.to_owned()),
                blob_id: blob.blob_id.clone(),
                source: entry.5,
            })
            .collect();
        let payload = ServerCommitPayload {
            commit: ServerCommit {
                commit_id: "commit_test".to_owned(),
                scope: ServerCommitScope::Project,
                org_id: "org_test".to_owned(),
                project_id: Some("prj_test".to_owned()),
                tree_id: "tree_test".to_owned(),
                parent_commit_id: None,
                version: 1,
                created_at: "2026-07-15T00:00:00Z".to_owned(),
            },
            tree: ServerTree {
                tree_id: "tree_test".to_owned(),
                entries: tree_entries,
            },
            blobs,
            project_org_selection: None,
        };

        materialize_payload(root.path(), "prj_test", &payload).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.path().join("cache/memory/spec/API.md")).unwrap(),
            "Context body"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("cache/memory/coding/STYLE.md")).unwrap(),
            "# Style\n\nApply while coding.\n\nRule body"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("cache/memory/workflow/CODING.md")).unwrap(),
            "# Coding\n\nWorkflow body\n\n1. Run tests"
        );
    }
}
