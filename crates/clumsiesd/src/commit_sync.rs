//! Commit synchronization, validated Ref checks, and atomic local generation publication.

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
/// Last validated remote check, distinct from local Ref installation.
const META_COMMIT_SYNC_LAST_CHECKED_SUCCESS_AT: &str = "commit_sync_last_checked_success_at";
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
/// Issue a document Base lease, or acknowledge one after acquiring its file lock.
pub struct DaemonCommitBasePinRequest {
    /// Exact immutable snapshot captured when the document was opened.
    pub commit_id: String,
    /// Omit to issue a provisional lease; provide the returned ID to acknowledge it.
    #[serde(default)]
    pub pin_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
/// Lease identity and private file that a document reader must keep locked.
///
/// Holding a shared OS lock protects the complete Base across daemon restarts.
/// An unlocked confirmed lease can be collected; its identifier alone is no pin.
pub struct DaemonCommitBasePin {
    /// Identity used for the acknowledgment request.
    pub pin_id: String,
    /// Immutable snapshot protected by this lease.
    pub commit_id: String,
    /// Open this file without following symlinks, then hold a shared lock until release.
    pub lock_path: String,
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
        "CREATE TABLE IF NOT EXISTS commit_base_pins (
            pin_id TEXT PRIMARY KEY,
            commit_id TEXT NOT NULL,
            issued_at INTEGER NOT NULL,
            confirmed INTEGER NOT NULL DEFAULT 0 CHECK (confirmed IN (0, 1))
        )",
    )
    .execute(pool)
    .await?;
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
    let scope = CommitRunScopeGuard { state };
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
    drop(scope);
    result
}

/// Clear the transient running scope on completion, cancellation, and unwinding.
struct CommitRunScopeGuard<'a> {
    /// State owning the scope; the caller holds the synchronization mutex for its lifetime.
    state: &'a DaemonState,
}

impl Drop for CommitRunScopeGuard<'_> {
    fn drop(&mut self) {
        match self.state.inner.commit_sync_run_scope.write() {
            Ok(mut scope) => *scope = CommitSyncRunScope::Idle,
            Err(_) => tracing::error!(
                event = "sync_scope_unavailable",
                "Commit sync scope cannot be cleared"
            ),
        }
    }
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
        last_checked_success_at: load_meta_value(
            &state.inner.pool,
            &commit_sync_meta_key(META_COMMIT_SYNC_LAST_CHECKED_SUCCESS_AT, scoped_project_id),
        )
        .await?,
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
    if load_cached_commit_payload(&state.inner.pool, commit_id)
        .await?
        .is_some()
    {
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
    let payload = tokio::task::spawn_blocking(move || {
        validate_commit_payload(&payload, &synthetic_state)?;
        Ok::<_, DaemonError>(payload)
    })
    .await
    .map_err(|error| DaemonError::Server(format!("Commit verification task failed: {error}")))??;
    let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
    cache_commit_payload(&mut tx, &payload).await?;
    tx.commit().await?;
    Ok(())
}

/// Issue or acknowledge a lease for a complete, verified document Base.
///
/// Acquires `sync_lock` before the storage write barrier to exclude Base GC. The
/// reader must acquire a shared OS file lock before acknowledgment and retain it
/// while using the Base; provisional unlocked leases have only a short grace period.
///
/// # Errors
/// Returns validation, download, verification, file or database errors. Acknowledgment
/// rejects missing, unlocked or mismatched leases rather than authorizing an unsafe edit.
pub(super) async fn retain_commit_base(
    state: &DaemonState,
    request: DaemonCommitBasePinRequest,
) -> Result<DaemonCommitBasePin, DaemonError> {
    validate_cache_component("commit_id", &request.commit_id)?;
    let _sync_guard = state.inner.sync_lock.lock().await;
    let _storage_guard = state.inner.storage_access.write().await;
    let pin_id = match request.pin_id {
        Some(pin_id) => {
            Uuid::parse_str(&pin_id).map_err(|_| expired_base_pin())?;
            let commit_id: Option<String> =
                sqlx::query_scalar("SELECT commit_id FROM commit_base_pins WHERE pin_id = $1")
                    .bind(&pin_id)
                    .fetch_optional(&state.inner.pool)
                    .await?;
            if commit_id.as_deref() != Some(request.commit_id.as_str())
                || !commit_pin_file_is_locked(&commit_pin_path(state, &pin_id))?
                || load_cached_commit_payload(&state.inner.pool, &request.commit_id)
                    .await?
                    .is_none()
            {
                return Err(expired_base_pin());
            }
            sqlx::query("UPDATE commit_base_pins SET confirmed = 1 WHERE pin_id = $1")
                .bind(&pin_id)
                .execute(&state.inner.pool)
                .await?;
            pin_id
        }
        None => {
            ensure_commit_cached(state, &request.commit_id).await?;
            let pin_id = Uuid::new_v4().to_string();
            let directory = state.inner.config.root_dir.join("commit-base-pins");
            super::project_storage::ensure_private_directory(&directory)?;
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let path = commit_pin_path(state, &pin_id);
            let file = options.open(&path)?;
            file.sync_all()?;
            if let Err(error) = sqlx::query(
                "INSERT INTO commit_base_pins (pin_id, commit_id, issued_at)
                 VALUES ($1, $2, CAST(strftime('%s', 'now') AS INTEGER))",
            )
            .bind(&pin_id)
            .bind(&request.commit_id)
            .execute(&state.inner.pool)
            .await
            {
                let _ = std::fs::remove_file(&path);
                return Err(error.into());
            }
            pin_id
        }
    };
    Ok(DaemonCommitBasePin {
        lock_path: commit_pin_path(state, &pin_id).display().to_string(),
        pin_id,
        commit_id: request.commit_id,
    })
}

/// Require the editor to reload when its lease can no longer authorize a captured Base.
fn expired_base_pin() -> DaemonError {
    DaemonError::State {
        code: "commit_base_pin_expired",
        message: "The document Base could not be retained; reload the document before editing."
            .to_owned(),
    }
}

/// Read a target from the immutable Project snapshot captured by the editor.
///
/// The caller holds `sync_lock` so GC cannot remove the Base between object reads.
/// Organization resources are resolved from that snapshot's selection, not a newer Ref.
///
/// # Errors
/// Returns database, verification, scope or missing-target errors; an unavailable
/// captured Base is never replaced with the current head.
pub(super) async fn load_captured_project_target(
    state: &DaemonState,
    project_id: &str,
    commit_id: &str,
    target_id: &str,
) -> Result<LoadedMemoryResource, DaemonError> {
    let payload = load_cached_commit_payload(&state.inner.pool, commit_id)
        .await?
        .ok_or_else(expired_base_pin)?;
    if payload.commit.scope != ServerCommitScope::Project
        || payload.commit.project_id.as_deref() != Some(project_id)
    {
        return Err(DaemonError::InvalidRequest(
            "The captured Base belongs to another Project".to_owned(),
        ));
    }
    let entry = payload
        .tree
        .entries
        .iter()
        .find(|entry| {
            entry.id == target_id && entry.kind != ServerTreeEntryKind::ProjectOrgSelection
        })
        .ok_or_else(|| {
            DaemonError::NotFound(format!(
                "Memory {target_id} is absent from the captured Base"
            ))
        })?;
    let blob = payload
        .blobs
        .iter()
        .find(|blob| blob.blob_id == entry.blob_id)
        .expect("the captured Base was verified complete");
    let path = entry.path.clone().expect("Memory path was verified");
    Ok(LoadedMemoryResource {
        is_directory: entry.is_directory,
        resource_id: entry.id.clone(),
        scope: match entry.scope {
            ServerTreeEntryScope::Org => SourceScope::Org,
            ServerTreeEntryScope::Project => SourceScope::Project,
            ServerTreeEntryScope::Daemon => return Err(expired_base_pin()),
        },
        kind: MemoryKind::Memory,
        title: path.clone(),
        path,
        description: entry.description.clone(),
        content_hash: content_hash(&blob.content),
        changed: true,
        content: Some(blob.content.clone()),
    })
}

/// Locate an already validated lease identifier inside the daemon's private pin directory.
fn commit_pin_path(state: &DaemonState, pin_id: &str) -> PathBuf {
    state
        .inner
        .config
        .root_dir
        .join("commit-base-pins")
        .join(format!("{pin_id}.lock"))
}

/// Probe an OS lease without following a symlink or accepting an unsafe pin directory.
///
/// # Errors
/// Returns file or lock errors. Missing or unsafe files are errors, not evidence that
/// a document released its Base, so collection can fail closed.
fn commit_pin_file_is_locked(path: &Path) -> Result<bool, DaemonError> {
    let directory = std::fs::symlink_metadata(path.parent().ok_or_else(expired_base_pin)?)?;
    if directory.file_type().is_symlink() || !directory.is_dir() {
        return Err(expired_base_pin());
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(expired_base_pin());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Capture live document roots and reap unlocked expired leases in the GC transaction.
///
/// The caller holds `sync_lock` and the storage write barrier. Live OS locks never
/// expire by age; expired provisional or unlocked confirmed rows are removed only
/// when this transaction commits. File cleanup follows that commit separately.
///
/// # Errors
/// Returns database or unsafe/missing pin-file errors before a cache sweep may commit.
pub(super) async fn live_commit_base_pins(
    state: &DaemonState,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<Vec<String>, DaemonError> {
    let rows: Vec<(String, String, bool, bool)> = sqlx::query_as(
        "SELECT pin_id, commit_id, confirmed,
                       CAST(strftime('%s', 'now') AS INTEGER) - issued_at < 60
                       FROM commit_base_pins",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut retained = Vec::new();
    for (pin_id, commit_id, confirmed, provisional) in rows {
        Uuid::parse_str(&pin_id).map_err(|_| expired_base_pin())?;
        // A live OS lock survives sleep and daemon restarts; it never expires by time.
        if commit_pin_file_is_locked(&commit_pin_path(state, &pin_id))?
            || (!confirmed && provisional)
        {
            retained.push(commit_id);
        } else {
            sqlx::query("DELETE FROM commit_base_pins WHERE pin_id = $1")
                .bind(&pin_id)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(retained)
}

/// Remove unlocked orphan lease files after the row-reaping transaction has committed.
///
/// The caller keeps `sync_lock` and the storage write barrier to exclude new leases.
/// Unknown filenames and files with a live OS lock are preserved.
///
/// # Errors
/// Returns database, directory or file errors without undoing the completed safe sweep.
pub(super) async fn cleanup_commit_base_pin_files(state: &DaemonState) -> Result<(), DaemonError> {
    let directory = state.inner.config.root_dir.join("commit-base-pins");
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
        Ok(_) => return Err(expired_base_pin()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let files = match std::fs::read_dir(directory) {
        Ok(files) => files,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in files {
        let entry = entry?;
        let Some(pin_id) = entry
            .path()
            .file_stem()
            .and_then(|id| id.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if Uuid::parse_str(&pin_id).is_err()
            || entry.path().extension().is_none_or(|ext| ext != "lock")
        {
            continue;
        }
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commit_base_pins WHERE pin_id = $1)")
                .bind(&pin_id)
                .fetch_one(&state.inner.pool)
                .await?;
        if !exists && !commit_pin_file_is_locked(&entry.path())? {
            std::fs::remove_file(entry.path())?;
        }
    }
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
        restore_missing_project_generation(state, &storage, commit_id.as_deref()).await?;
    }
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
    let verification_root = generation.clone();
    let verification_project = request.project_id.clone();
    let verification_commit = commit_id.clone();
    let verified = tokio::task::spawn_blocking(move || {
        verify_generation_ref(
            &verification_root,
            &verification_project,
            verification_commit.as_deref(),
        )
    })
    .await
    .map_err(|error| DaemonError::Server(format!("Commit verification task failed: {error}")))?;
    if let Err(error) = verified {
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
    if !generation.exists() {
        restore_missing_project_generation(state, &storage, commit_id.as_deref()).await?;
    }
    let project_id = request.project_id.clone();
    let checkout_commit_id = commit_id.clone();
    let checkout = tokio::task::spawn_blocking(move || {
        load_project_checkout(&generation, &project_id, checkout_commit_id.as_deref())
    })
    .await
    .map_err(|error| DaemonError::Server(format!("Project snapshot task failed: {error}")))??;
    let Some(mut checkout) = checkout else {
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
    };
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
    let cycle_request_id = crate::diagnostics::request_id();
    for project_id in ready_project_ids {
        let state = state.clone();
        let project_gate = project_gate.clone();
        project_tasks.spawn(crate::diagnostics::REQUEST_ID.scope(
            cycle_request_id.clone(),
            async move {
                let result = async {
                    let _permit = project_gate.acquire_owned().await.map_err(|_| {
                        DaemonError::Server("Project Commit-state request gate closed".to_owned())
                    })?;
                    fetch_project_ref(&state, &project_id).await
                }
                .await;
                (project_id, result)
            },
        ));
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
    let observation = crate::performance::start("commit_check");
    let result = async {
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
    .await;
    let trusted = result.as_ref().ok().is_some_and(|(remote, _)| {
        crate::project_access::current(state)
            .is_some_and(|access| access.org_id == remote.reference.org_id)
    });
    if trusted {
        let record = crate::performance::observe(
            "commit_check_record",
            upsert_meta_timestamp(
                &state.inner.pool,
                &commit_sync_meta_key(META_COMMIT_SYNC_LAST_CHECKED_SUCCESS_AT, Some(project_id)),
            ),
        )
        .await;
        if let Err(error) = record {
            tracing::warn!(event = "commit_check_record_failed", request_id = %crate::diagnostics::request_id(), kind = crate::diagnostics::error_kind(&error));
        }
    }
    let outcome = match &result {
        Ok(_) if !trusted => "superseded",
        Ok((remote, _)) if remote.update_available => "updated",
        Ok(_) => "no_change",
        Err(_) => "failed",
    };
    let elapsed_us = observation.finish(outcome);
    tracing::info!(event = "commit_check_completed", request_id = %crate::diagnostics::request_id(), project_id, outcome, elapsed_us);
    result
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
    let observation = crate::performance::start("commit_install");
    let mut changed = false;
    let result = async {
        let previous_commit_id = match materialized_project_id {
            Some(project_id) => {
                load_ref_commit(&state.inner.pool, &project_ref_key(project_id)).await?
            }
            None => None,
        };
        let remote_commit_id = commit_state.reference.commit_id.as_deref();
        if let Some(commit_id) = remote_commit_id {
            validate_cache_component("commit_id", commit_id)?;
            let cached_payload = load_cached_commit_payload(&state.inner.pool, commit_id).await?;
            let generation_payload = match materialized_project_id {
                Some(project_id) => {
                    let storage = super::project_storage::resolve_active(state, project_id).await?;
                    let root = storage.generation_path(commit_id);
                    let project_id = project_id.to_owned();
                    let commit_id = commit_id.to_owned();
                    tokio::task::spawn_blocking(move || {
                        verify_generation_ref(&root, &project_id, Some(&commit_id))
                    })
                    .await
                    .map_err(|error| {
                        DaemonError::Server(format!("Commit verification task failed: {error}"))
                    })?
                    .ok()
                    .flatten()
                }
                None => None,
            };
            let generation_ready =
                materialized_project_id.is_none() || generation_payload.is_some();
            if cached_payload.is_some() && generation_ready {
                let _storage_guard = state.inner.storage_access.write().await;
                let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
                changed = upsert_ref(&mut tx, &commit_state.reference, etag).await?;
                tx.commit().await?;
                if let Some(project_id) = materialized_project_id {
                    prune_project_generations(
                        state,
                        project_id,
                        Some(commit_id),
                        previous_commit_id.as_deref(),
                    )
                    .await;
                }
                state.inner.search_index_notify.notify_one();
                return Ok(());
            }

            let payload = match cached_payload.or(generation_payload) {
                Some(payload) => payload,
                None => {
                    crate::performance::observe(
                        "commit_download",
                        get_server_json(state, &format!("/api/v1/commits/{commit_id}")),
                    )
                    .await?
                }
            };
            let validation_state = commit_state.clone();
            let payload = tokio::task::spawn_blocking(move || {
                let observation = crate::performance::start("commit_validate");
                let result = (|| {
                    validate_commit_payload(&payload, &validation_state)?;
                    Ok::<_, DaemonError>(payload)
                })();
                observation.finish_result(&result);
                result
            })
            .await
            .map_err(|error| {
                DaemonError::Server(format!("Commit verification task failed: {error}"))
            })??;
            let _storage_guard = state.inner.storage_access.write().await;
            let mut installed_location_revision = None;
            if let Some(project_id) = materialized_project_id {
                let storage = super::project_storage::resolve_active(state, project_id).await?;
                installed_location_revision = Some(storage.location_revision);
                let managed_root = storage.managed_root;
                let project_id = project_id.to_owned();
                let materialized_payload = payload.clone();
                tokio::task::spawn_blocking(move || {
                    ensure_project_generation(
                        &managed_root,
                        &project_id,
                        &materialized_payload,
                        true,
                    )
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
            changed = upsert_ref(&mut tx, &commit_state.reference, etag).await?;
            tx.commit().await?;
            if let Some(project_id) = materialized_project_id {
                prune_project_generations(
                    state,
                    project_id,
                    Some(commit_id),
                    previous_commit_id.as_deref(),
                )
                .await;
            }
            state.inner.search_index_notify.notify_one();
        } else {
            let _storage_guard = state.inner.storage_access.write().await;
            if let Some(project_id) = materialized_project_id {
                let storage = super::project_storage::resolve_active(state, project_id).await?;
                let managed_root = storage.managed_root;
                let project_id = project_id.to_owned();
                tokio::task::spawn_blocking(move || {
                    ensure_empty_generation(&managed_root, &project_id, true)
                })
                .await
                .map_err(|error| {
                    DaemonError::Server(format!("Commit materialization task failed: {error}"))
                })??;
            }
            let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
            changed = upsert_ref(&mut tx, &commit_state.reference, etag).await?;
            tx.commit().await?;
            if let Some(project_id) = materialized_project_id {
                prune_project_generations(state, project_id, None, previous_commit_id.as_deref())
                    .await;
            }
            state.inner.search_index_notify.notify_one();
        }
        Ok(())
    }
    .await;
    let outcome = if result.is_err() {
        "failed"
    } else if changed {
        "updated"
    } else {
        "no_change"
    };
    let elapsed_us = observation.finish(outcome);
    tracing::info!(event = "commit_install_completed", request_id = %crate::diagnostics::request_id(), outcome, elapsed_us);
    result
}

/// Retain published generations, live reader leases and unfinished Draft dependencies.
///
/// Call with `sync_lock` and the storage write barrier held, after Ref publication.
/// An immediate transaction stays open through directory deletion so Draft insertion
/// cannot cross the roots snapshot. Failed lease reads abort before any file sweep;
/// maintenance failures never withdraw the already committed Ref.
async fn prune_project_generations(
    state: &DaemonState,
    project_id: &str,
    current_commit_id: Option<&str>,
    previous_commit_id: Option<&str>,
) {
    let result = async {
        let storage = super::project_storage::resolve_active(state, project_id).await?;
        let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut retained = sqlx::query_scalar::<_, String>(
            "SELECT base_commit_id FROM local_drafts d
             WHERE project_id = $1 AND base_commit_id IS NOT NULL
               AND (status IN ('open', 'submitted') OR EXISTS (
                   SELECT 1 FROM local_draft_operations o
                   WHERE o.draft_id = d.draft_id AND o.sync_status <> 'synced'))
             UNION SELECT current_commit_id FROM local_drafts d
             WHERE project_id = $1 AND current_commit_id IS NOT NULL
               AND (status IN ('open', 'submitted') OR EXISTS (
                   SELECT 1 FROM local_draft_operations o
                   WHERE o.draft_id = d.draft_id AND o.sync_status <> 'synced'))",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
        retained.extend(live_commit_base_pins(state, &mut tx).await?);
        let current = current_commit_id.unwrap_or(EMPTY_GENERATION).to_owned();
        retained.insert(current.clone());
        let saved_previous: Option<String> =
            sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = $1")
                .bind(format!("previous_project_generation:{project_id}"))
                .fetch_optional(&mut *tx)
                .await?;
        let previous = previous_commit_id
            .filter(|previous| *previous != current)
            .map(str::to_owned)
            .or(saved_previous);
        let previous = match previous {
            Some(previous) => Some(previous),
            None => sqlx::query_scalar::<_, Option<String>>(
                "SELECT parent_commit_id FROM cached_commits WHERE commit_id = $1",
            )
            .bind(&current)
            .fetch_optional(&mut *tx)
            .await?
            .flatten(),
        };
        if let Some(previous) = previous {
            retained.insert(previous);
        }
        tokio::task::spawn_blocking(move || {
            prune_generation_directories(&storage.managed_root, &storage.project_id, &retained)
        })
        .await
        .map_err(|error| {
            DaemonError::Server(format!("Commit cache cleanup task failed: {error}"))
        })??;
        tx.commit().await?;
        cleanup_commit_base_pin_files(state).await
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(%project_id, "published Commit cache cleanup failed: {error}");
    }
}

/// Delete only verified daemon generations and abandoned daemon staging directories.
///
/// The caller holds the storage write barrier so readers cannot lose a generation.
/// Unrecognized or corrupt generation directories are preserved for investigation.
///
/// # Errors
/// Returns directory or deletion errors; generations already removed stay removed.
fn prune_generation_directories(
    managed_root: &Path,
    project_id: &str,
    retained: &BTreeSet<String>,
) -> Result<(), DaemonError> {
    for entry in std::fs::read_dir(managed_root.join("generations"))? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if retained.contains(&name) || !entry.file_type()?.is_dir() {
            continue;
        }
        let abandoned = [".tmp-", ".corrupt-"].iter().any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|id| Uuid::parse_str(id).is_ok())
        });
        if abandoned
            || verify_generation_ref(
                &entry.path(),
                project_id,
                (name != EMPTY_GENERATION).then_some(name.as_str()),
            )
            .is_ok()
        {
            std::fs::remove_dir_all(entry.path())?;
        }
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
    let project_id = project_id.to_owned();
    tokio::task::spawn_blocking(move || {
        verify_generation_ref(&generation, &project_id, commit_id.as_deref()).map(|_| ())
    })
    .await
    .map_err(|error| DaemonError::Server(format!("Commit verification task failed: {error}")))?
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

    validate_commit_objects(payload)
}

/// Verify Blob addresses, Tree ownership/paths and the exact selected-organization Blob.
///
/// # Errors
/// Returns payload-integrity errors for missing, duplicate, unreferenced or inconsistent
/// objects. Validation changes neither incoming Blob bytes nor stored historical objects.
fn validate_commit_objects(payload: &ServerCommitPayload) -> Result<(), DaemonError> {
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

    let selection_blob_id = validate_commit_tree(
        &payload.tree,
        payload.commit.scope,
        payload.commit.project_id.as_deref(),
    )?;
    let mut referenced_blobs = BTreeSet::new();
    for entry in &payload.tree.entries {
        if !blobs.contains_key(entry.blob_id.as_str()) {
            return Err(DaemonError::Server(format!(
                "Tree entry {} references a missing Blob",
                entry.id
            )));
        }
        if entry.is_directory
            && (entry.kind != ServerTreeEntryKind::Memory
                || !blobs[entry.blob_id.as_str()].content.is_empty())
        {
            return Err(DaemonError::Server(
                "directory entries must be memories with empty content".to_owned(),
            ));
        }
        referenced_blobs.insert(entry.blob_id.as_str());
        if entry.kind != ServerTreeEntryKind::ProjectOrgSelection {
            materialized_resource_content(entry.kind, &blobs[entry.blob_id.as_str()].content)?;
        }
    }
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

/// Validate a Tree's ownership and portable paths, returning its selection Blob identity.
///
/// # Errors
/// Returns invalid-ownership, duplicate-entry, directory or conflicting-path errors.
fn validate_commit_tree<'a>(
    tree: &'a ServerTree,
    scope: ServerCommitScope,
    project_id: Option<&str>,
) -> Result<Option<&'a str>, DaemonError> {
    let mut entry_ids = BTreeSet::new();
    let mut selection_blob_id = None;
    for entry in &tree.entries {
        if !entry_ids.insert(entry.id.as_str()) {
            return Err(DaemonError::Server(format!(
                "Tree contains duplicate entry {}",
                entry.id,
            )));
        }
        validate_tree_entry_ownership(entry, scope, project_id)?;
        if entry.is_directory && entry.kind != ServerTreeEntryKind::Memory {
            return Err(DaemonError::Server(
                "directory entries must be memories with empty content".to_owned(),
            ));
        }
        if entry.kind == ServerTreeEntryKind::ProjectOrgSelection
            && selection_blob_id.replace(entry.blob_id.as_str()).is_some()
        {
            return Err(DaemonError::Server(
                "Project Commit contains more than one organization selection".to_owned(),
            ));
        }
    }
    validate_materialization_paths(&tree.entries)?;
    match scope {
        ServerCommitScope::Project if project_id.is_none() || selection_blob_id.is_none() => {
            return Err(DaemonError::Server(
                "Project Commit is missing its organization selection or project_id".to_owned(),
            ));
        }
        ServerCommitScope::Org if selection_blob_id.is_some() => {
            return Err(DaemonError::Server(
                "Organization Commit contains a project organization selection".to_owned(),
            ));
        }
        _ => {}
    }
    Ok(selection_blob_id)
}

fn validate_tree_entry_ownership(
    entry: &ServerTreeEntry,
    scope: ServerCommitScope,
    project_id: Option<&str>,
) -> Result<(), DaemonError> {
    let valid = match (scope, entry.kind, entry.scope) {
        (
            ServerCommitScope::Project,
            ServerTreeEntryKind::ProjectOrgSelection,
            ServerTreeEntryScope::Daemon,
        ) => {
            entry.project_id.as_deref() == project_id
                && entry.path.is_none()
                && entry.source == ServerTreeEntrySource::Config
        }
        (ServerCommitScope::Project, _, ServerTreeEntryScope::Project) => {
            entry.project_id.as_deref() == project_id
                && entry.source == ServerTreeEntrySource::Project
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
    let mut paths = BTreeMap::<String, (String, String, bool)>::new();
    for entry in entries {
        if entry.kind == ServerTreeEntryKind::ProjectOrgSelection {
            continue;
        }
        validate_resource_path(entry)?;
        let output_path = materialization_output_path(entry)?;
        insert_materialization_path(&mut paths, &entry.id, &output_path, entry.is_directory)?;
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
    paths: &mut BTreeMap<String, (String, String, bool)>,
    entry_id: &str,
    output_path: &str,
    is_directory: bool,
) -> Result<(), DaemonError> {
    let normalized = output_path.to_lowercase();
    if let Some((existing_id, existing_path, _)) = paths.get(&normalized) {
        return Err(DaemonError::Server(format!(
            "Tree materializes {existing_id} at {existing_path} and {entry_id} at {output_path}, which conflict"
        )));
    }
    for (index, _) in normalized.rmatch_indices('/') {
        if let Some((existing_id, existing_path, false)) = paths.get(&normalized[..index]) {
            return Err(DaemonError::Server(format!(
                "Tree materializes {existing_id} at {existing_path} and {entry_id} at {output_path}, which conflict"
            )));
        }
    }
    let descendant_prefix = format!("{normalized}/");
    if let Some((_, (existing_id, existing_path, _))) = paths
        .range(descendant_prefix.clone()..)
        .next()
        .filter(|(path, _)| !is_directory && path.starts_with(&descendant_prefix))
    {
        return Err(DaemonError::Server(format!(
            "Tree materializes {existing_id} at {existing_path} and {entry_id} at {output_path}, which conflict"
        )));
    }
    paths.insert(
        normalized,
        (entry_id.to_owned(), output_path.to_owned(), is_directory),
    );
    Ok(())
}

/// Admit verified immutable objects or repair corrupt cached projections in one transaction.
///
/// The caller validates the incoming payload and owns commit/rollback. Blob content
/// is stored verbatim; scalar Commit fields and header JSON are repaired together.
///
/// # Errors
/// Returns database errors or a valid immutable-object collision before commit.
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
                if blob_object_id(&content) == blob.blob_id {
                    return Err(DaemonError::Server(format!(
                        "Cached Blob {} violates immutability",
                        blob.blob_id
                    )));
                }
                sqlx::query("UPDATE cached_blobs SET content = $2 WHERE blob_id = $1")
                    .bind(&blob.blob_id)
                    .bind(&blob.content)
                    .execute(&mut **tx)
                    .await?;
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
            if let Ok(existing) = serde_json::from_str::<ServerTree>(&value)
                && existing.tree_id == payload.tree.tree_id
                && existing != payload.tree
            {
                let owners: Vec<(String, Option<String>, String)> = sqlx::query_as(
                    "SELECT scope, project_id, payload_json FROM cached_commits WHERE tree_id = $1",
                )
                .bind(&existing.tree_id)
                .fetch_all(&mut **tx)
                .await?;
                // Missing Blob bytes do not make an immutable Tree's metadata corrupt.
                let valid = owners.is_empty()
                    || owners.iter().any(|(scope, project_id, header_json)| {
                        if let Ok(header) = serde_json::from_str::<ServerCommit>(header_json)
                            && header.tree_id == existing.tree_id
                            && validate_commit_tree(
                                &existing,
                                header.scope,
                                header.project_id.as_deref(),
                            )
                            .is_ok()
                        {
                            return true;
                        }
                        let scope = match scope.as_str() {
                            "org" => ServerCommitScope::Org,
                            "project" => ServerCommitScope::Project,
                            _ => return false,
                        };
                        validate_commit_tree(&existing, scope, project_id.as_deref()).is_ok()
                    });
                if valid {
                    return Err(DaemonError::Server(format!(
                        "Cached Tree {} violates immutability",
                        payload.tree.tree_id,
                    )));
                }
            }
            sqlx::query("UPDATE cached_trees SET payload_json = $2 WHERE tree_id = $1")
                .bind(&payload.tree.tree_id)
                .bind(&tree_json)
                .execute(&mut **tx)
                .await?;
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
    let existing: Option<CachedCommitRecord> = sqlx::query_as(
        "SELECT commit_id, scope, org_id, project_id, tree_id, parent_commit_id,
                version, created_at, payload_json FROM cached_commits WHERE commit_id = $1",
    )
    .bind(&payload.commit.commit_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(existing) = &existing {
        if let Ok(header) = serde_json::from_str::<ServerCommit>(&existing.payload_json)
            && header.commit_id == payload.commit.commit_id
            && header.version > 0
            && header.parent_commit_id.as_deref() != Some(header.commit_id.as_str())
            && header != payload.commit
        {
            return Err(DaemonError::Server(format!(
                "Cached Commit {} violates immutability",
                payload.commit.commit_id
            )));
        }
        if existing.matches(&payload.commit) && existing.payload_json == commit_json {
            return Ok(());
        }
    }
    // Header JSON and scalar lookup columns form one cached object. Repair all
    // columns together, including when only the scalar projection was damaged.
    sqlx::query(
        "INSERT INTO cached_commits (
            commit_id, scope, org_id, project_id, tree_id,
            parent_commit_id, version, created_at, payload_json
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT(commit_id) DO UPDATE SET
            scope = excluded.scope, org_id = excluded.org_id,
            project_id = excluded.project_id, tree_id = excluded.tree_id,
            parent_commit_id = excluded.parent_commit_id, version = excluded.version,
            created_at = excluded.created_at, payload_json = excluded.payload_json",
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
    Ok(())
}

async fn upsert_ref(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    reference: &ServerRef,
    etag: &str,
) -> Result<bool, DaemonError> {
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
    if ref_changed
        && reference.scope == ServerCommitScope::Project
        && let Some(previous) = previous_commit_id.as_ref()
    {
        let project_id = reference
            .project_id
            .as_deref()
            .expect("Project Ref ownership was validated");
        upsert_meta_value(
            tx,
            &format!("previous_project_generation:{project_id}"),
            Some(previous.as_deref().unwrap_or(EMPTY_GENERATION)),
        )
        .await?;
    }
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
                 WHERE project_id = $1 AND resource_scope = 'project'
                   AND (status IN ('open', 'submitted') OR EXISTS (
                       SELECT 1 FROM local_draft_operations o
                       WHERE o.draft_id = local_drafts.draft_id AND o.sync_status <> 'synced'
                   ))",
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
                   AND (status IN ('open', 'submitted') OR EXISTS (
                       SELECT 1 FROM local_draft_operations o
                       WHERE o.draft_id = local_drafts.draft_id AND o.sync_status <> 'synced'
                   ))
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
    Ok(ref_changed)
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
                 WHERE resource_scope = 'project' AND project_id = $1
                   AND (status IN ('open', 'submitted') OR EXISTS (
                       SELECT 1 FROM local_draft_operations o
                       WHERE o.draft_id = local_drafts.draft_id AND o.sync_status <> 'synced'
                   ))",
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
                   AND (d.status IN ('open', 'submitted') OR EXISTS (
                       SELECT 1 FROM local_draft_operations o
                       WHERE o.draft_id = d.draft_id AND o.sync_status <> 'synced'
                   ))
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
    if !matches!(resource_kind, "memory" | "context" | "rule" | "workflow") {
        return Err(DaemonError::Server(format!(
            "unknown Draft resource kind {resource_kind}"
        )));
    }
    // The mutable Draft type is canonical Memory. Immutable historical Trees
    // keep their original Context/Rule/Workflow tags and retain the same target.
    Ok(tree
        .entries
        .into_iter()
        .find(|entry| {
            entry.id == target_id
                && matches!(
                    entry.kind,
                    ServerTreeEntryKind::Memory
                        | ServerTreeEntryKind::Context
                        | ServerTreeEntryKind::Rule
                        | ServerTreeEntryKind::Workflow
                )
        })
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

/// Load a complete cached snapshot and verify its objects away from the async worker.
///
/// Incomplete or corrupt derived content is a cache miss; database access failures
/// are preserved. Callers exclude GC while consuming the returned Base.
///
/// # Errors
/// Returns database or verification-task errors, rather than treating them as misses.
async fn load_cached_commit_payload(
    pool: &SqlitePool,
    commit_id: &str,
) -> Result<Option<ServerCommitPayload>, DaemonError> {
    let record: Option<CachedCommitRecord> = sqlx::query_as(
        "SELECT commit_id, scope, org_id, project_id, tree_id, parent_commit_id,
                version, created_at, payload_json FROM cached_commits WHERE commit_id = $1",
    )
    .bind(commit_id)
    .fetch_optional(pool)
    .await?;
    let Some(record) = record else {
        return Ok(None);
    };
    let Ok(commit) = serde_json::from_str::<ServerCommit>(&record.payload_json) else {
        return Ok(None);
    };
    if !record.matches(&commit) {
        return Ok(None);
    }
    let tree_json: Option<String> =
        sqlx::query_scalar("SELECT payload_json FROM cached_trees WHERE tree_id = $1")
            .bind(&commit.tree_id)
            .fetch_optional(pool)
            .await?;
    let Some(tree_json) = tree_json else {
        return Ok(None);
    };
    cached_tree_payload(pool, commit, &tree_json).await
}

/// Assemble a Tree's exact Blob set and validate its complete immutable payload.
///
/// # Errors
/// Returns database or verification-task errors. Invalid content yields a cache miss.
async fn cached_tree_payload<'a>(
    executor: impl sqlx::Executor<'a, Database = sqlx::Sqlite>,
    commit: ServerCommit,
    tree_json: &str,
) -> Result<Option<ServerCommitPayload>, DaemonError> {
    let Ok(tree) = serde_json::from_str::<ServerTree>(tree_json) else {
        return Ok(None);
    };
    if tree.tree_id != commit.tree_id
        || commit.version < 1
        || commit.parent_commit_id.as_deref() == Some(commit.commit_id.as_str())
    {
        return Ok(None);
    }
    let rows = sqlx::query(
        "SELECT DISTINCT b.blob_id, b.content
         FROM cached_blobs b
         JOIN json_each($1, '$.entries') e ON b.blob_id = json_extract(e.value, '$.blob_id')
         ORDER BY b.blob_id",
    )
    .bind(tree_json)
    .fetch_all(executor)
    .await?;
    let blobs = rows
        .into_iter()
        .map(|row| {
            Ok(ServerBlob {
                blob_id: row.try_get("blob_id")?,
                content: row.try_get("content")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let project_org_selection = tree
        .entries
        .iter()
        .find(|entry| entry.kind == ServerTreeEntryKind::ProjectOrgSelection)
        .and_then(|entry| blobs.iter().find(|blob| blob.blob_id == entry.blob_id))
        .map(|blob| serde_json::from_str(&blob.content))
        .transpose();
    let Ok(project_org_selection) = project_org_selection else {
        return Ok(None);
    };
    let payload = ServerCommitPayload {
        commit,
        tree,
        blobs,
        project_org_selection,
    };
    tokio::task::spawn_blocking(move || {
        validate_commit_objects(&payload).ok()?;
        Some(payload)
    })
    .await
    .map_err(|error| {
        DaemonError::Server(format!("Cached Commit verification task failed: {error}"))
    })
}

/// Rebuild a missing generation offline from a complete central snapshot.
///
/// The caller holds the storage barrier. If central objects are unavailable, request
/// synchronization and leave the generation missing; no replacement head is invented.
///
/// # Errors
/// Returns identity, database, filesystem or verification errors without replacing a
/// corrupt existing generation or deleting local Draft operations.
async fn restore_missing_project_generation(
    state: &DaemonState,
    storage: &super::project_storage::ActiveProjectStorage,
    commit_id: Option<&str>,
) -> Result<(), DaemonError> {
    let payload = match commit_id {
        Some(commit_id) => {
            let Some(payload) = load_cached_commit_payload(&state.inner.pool, commit_id).await?
            else {
                state.request_sync();
                return Ok(());
            };
            if payload.commit.commit_id != commit_id
                || payload.commit.scope != ServerCommitScope::Project
                || payload.commit.project_id.as_deref() != Some(storage.project_id.as_str())
                || payload.commit.tree_id != payload.tree.tree_id
            {
                return Err(DaemonError::Server(
                    "Cached Commit identity does not match its Ref".to_owned(),
                ));
            }
            Some(payload)
        }
        None => None,
    };
    let storage = storage.clone();
    tokio::task::spawn_blocking(move || match payload {
        Some(payload) => {
            ensure_project_generation(&storage.managed_root, &storage.project_id, &payload, false)
        }
        None => ensure_empty_generation(&storage.managed_root, &storage.project_id, false),
    })
    .await
    .map_err(|error| {
        DaemonError::Server(format!("Commit cache recovery task failed: {error}"))
    })??;
    Ok(())
}

fn ensure_project_generation(
    managed_root: &Path,
    project_id: &str,
    payload: &ServerCommitPayload,
    repair_corrupt: bool,
) -> Result<PathBuf, DaemonError> {
    let marker = serde_json::to_vec(payload)?;
    ensure_generation(
        managed_root,
        project_id,
        &payload.commit.commit_id,
        &marker,
        repair_corrupt,
        |root| materialize_payload(root, project_id, payload),
    )
}

/// Materialize a verified empty Ref through the same guarded publication path as a Commit.
///
/// # Errors
/// Returns path, file or verification errors; corrupt existing directories are replaced
/// only when the caller explicitly permits repair and a complete candidate is ready.
fn ensure_empty_generation(
    managed_root: &Path,
    project_id: &str,
    repair_corrupt: bool,
) -> Result<PathBuf, DaemonError> {
    let marker = serde_json::to_vec(&json!({
        "project_id": project_id,
        "commit_id": null
    }))?;
    ensure_generation(
        managed_root,
        project_id,
        EMPTY_GENERATION,
        &marker,
        repair_corrupt,
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
    repair_corrupt: bool,
    materialize: impl FnOnce(&Path) -> Result<(), DaemonError>,
) -> Result<PathBuf, DaemonError> {
    validate_cache_component("project_id", project_id)?;
    validate_cache_component("generation", generation)?;
    let generations = managed_root.join("generations");
    super::project_storage::ensure_private_directory(&generations)?;
    let final_root = generations.join(generation);
    match std::fs::symlink_metadata(&final_root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(DaemonError::Server(format!(
                    "Materialized generation {} is not a safe directory",
                    final_root.display()
                )));
            }
            let verified = verify_generation_marker(&final_root, marker).and_then(|()| {
                verify_generation_ref(
                    &final_root,
                    project_id,
                    (generation != EMPTY_GENERATION).then_some(generation),
                )
            });
            match verified {
                Ok(_) => return Ok(final_root),
                Err(error) if !repair_corrupt => return Err(error),
                Err(_) => {}
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let temporary_root = generations.join(format!(".tmp-{}", Uuid::new_v4().simple()));
    super::project_storage::ensure_private_directory(&temporary_root)?;
    let build_result = (|| {
        materialize(&temporary_root)?;
        super::project_storage::write_private_file(
            &temporary_root.join("commit-payload.json"),
            marker,
        )?;
        super::project_storage::secure_managed_tree(&temporary_root)?;
        verify_generation_ref(
            &temporary_root,
            project_id,
            (generation != EMPTY_GENERATION).then_some(generation),
        )?;
        Ok::<(), DaemonError>(())
    })();
    if let Err(error) = build_result {
        let _ = std::fs::remove_dir_all(&temporary_root);
        return Err(error);
    }

    let displaced = generations.join(format!(".corrupt-{}", Uuid::new_v4().simple()));
    if repair_corrupt
        && final_root.exists()
        && let Err(error) = std::fs::rename(&final_root, &displaced)
    {
        let _ = std::fs::remove_dir_all(&temporary_root);
        return Err(error.into());
    }
    let result = match std::fs::rename(&temporary_root, &final_root) {
        Ok(()) => Ok(final_root.clone()),
        Err(_error) if final_root.exists() => {
            let _ = std::fs::remove_dir_all(&temporary_root);
            verify_generation_marker(&final_root, marker).map(|()| final_root.clone())
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temporary_root);
            Err(error.into())
        }
    };
    if result.is_err() && displaced.exists() && !final_root.exists() {
        std::fs::rename(&displaced, &final_root)?;
    }
    if result.is_ok() && displaced.exists() {
        std::fs::remove_dir_all(&displaced)?;
    }
    #[cfg(unix)]
    if result.is_ok() {
        std::fs::File::open(&generations)?.sync_all()?;
    }
    result
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
) -> Result<Option<ServerCommitPayload>, DaemonError> {
    let metadata = std::fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(DaemonError::Server(format!(
            "Materialized generation {} is missing",
            root.display()
        )));
    }
    for name in ["manifest.json", "commit-payload.json"] {
        let path = root.join(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(DaemonError::Server(format!(
                "Materialized generation file {} is not safe",
                path.display()
            )));
        }
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
                || manifest.get("tree_id").and_then(|value| value.as_str())
                    != Some(payload.tree.tree_id.as_str())
            {
                return Err(DaemonError::Server(format!(
                    "Materialized generation {} has an invalid Commit marker",
                    root.display()
                )));
            }
            validate_commit_objects(&payload)?;
            Ok(Some(payload))
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
            Ok(None)
        }
    }
}

fn load_project_checkout(
    root: &Path,
    project_id: &str,
    commit_id: Option<&str>,
) -> Result<Option<DaemonProjectCheckout>, DaemonError> {
    let payload = match verify_generation_ref(root, project_id, commit_id) {
        Ok(payload) => payload,
        Err(_) => return Ok(None),
    };
    let Some(commit_id) = commit_id else {
        return Ok(Some(DaemonProjectCheckout {
            project_id: project_id.to_owned(),
            commit_id: None,
            ref_etag: None,
            commit_created_at: None,
            org_selection_revision: 0,
            selected_org_resource_ids: Vec::new(),
            resources: Vec::new(),
            ready: true,
        }));
    };

    let payload = payload.expect("verified non-empty Project Commit has a payload");
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

    let selected_org_resource_ids: BTreeSet<String> = selection
        .get("memories")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| DaemonError::Server("Project selection is missing memories".to_owned()))?
        .iter()
        .map(|memory| {
            memory
                .get("memory_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    DaemonError::Server("Project selection has no Memory identity".to_owned())
                })
        })
        .collect::<Result<_, _>>()?;
    let mut resources = Vec::new();
    for entry in &payload.tree.entries {
        let scope = match entry.scope {
            ServerTreeEntryScope::Org => DaemonDraftScope::Org,
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
            content: DaemonDraftContent {
                org_source: entry.org_source.clone(),
                is_directory: entry.is_directory,
                ..project_checkout_content(entry.kind, &blob.content)?
            },
        });
    }

    Ok(Some(DaemonProjectCheckout {
        project_id: project_id.to_owned(),
        commit_id: Some(commit_id.to_owned()),
        ref_etag: None,
        commit_created_at: Some(payload.commit.created_at),
        org_selection_revision,
        selected_org_resource_ids: selected_org_resource_ids.into_iter().collect(),
        resources,
        ready: true,
    }))
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
            org_source: None,
            is_directory: false,
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
        if entry.is_directory {
            if !blob.content.is_empty() {
                return Err(DaemonError::Server(
                    "directory content must be empty".to_owned(),
                ));
            }
            std::fs::create_dir_all(root.join(materialization_output_path(entry)?))?;
            continue;
        }
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

#[derive(sqlx::FromRow)]
/// Scalar lookup columns and header JSON that must describe the same cached Commit.
struct CachedCommitRecord {
    commit_id: String,
    scope: String,
    org_id: String,
    project_id: Option<String>,
    tree_id: String,
    parent_commit_id: Option<String>,
    version: i64,
    created_at: String,
    payload_json: String,
}

impl CachedCommitRecord {
    /// Reject scalar lookup projections that disagree with their immutable header JSON.
    fn matches(&self, header: &ServerCommit) -> bool {
        self.commit_id == header.commit_id
            && self.scope == header.scope.as_str()
            && self.org_id == header.org_id
            && self.project_id == header.project_id
            && self.tree_id == header.tree_id
            && self.parent_commit_id == header.parent_commit_id
            && self.version == header.version
            && self.created_at == header.created_at
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    org_source: Option<crate::types::OrgMemorySource>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    is_directory: bool,
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

    #[tokio::test]
    async fn upstream_refresh_ignores_terminal_drafts_and_checks_active_drafts() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::migration::migrate_local_db(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO cached_refs
             (ref_key, name, scope, org_id, project_id, etag, server_updated_at)
             VALUES ('project:test', 'main', 'project', 'org_test', 'prj_test', 'test', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let tree = ServerTree {
            tree_id: "tree_current".to_owned(),
            entries: vec![ServerTreeEntry {
                org_source: None,
                is_directory: false,
                id: "mem_test".to_owned(),
                kind: ServerTreeEntryKind::Memory,
                scope: ServerTreeEntryScope::Project,
                project_id: Some("prj_test".to_owned()),
                description: String::new(),
                path: Some("guide.md".to_owned()),
                blob_id: "blob_current".to_owned(),
                source: ServerTreeEntrySource::Project,
            }],
        };
        sqlx::query("INSERT INTO cached_trees (tree_id, payload_json) VALUES ($1, $2)")
            .bind(&tree.tree_id)
            .bind(serde_json::to_string(&tree).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO cached_commits
             (commit_id, scope, org_id, project_id, tree_id, version, created_at, payload_json)
             VALUES ('commit_current', 'project', 'org_test', 'prj_test', 'tree_current', 1, 'now', '{}')",
        ).execute(&pool).await.unwrap();
        for scope in ["project", "org"] {
            for status in ["open", "submitted", "merged", "discarded"] {
                let base =
                    matches!(status, "merged" | "discarded").then_some("uncached_old_commit");
                sqlx::query(
                    "INSERT INTO local_drafts
                     (draft_id, project_id, resource_scope, resource_kind, target_id,
                      status, base_commit_id, current_commit_id)
                     VALUES ($1, 'prj_test', $2, 'memory', 'mem_test', $3, $4, 'commit_current')",
                )
                .bind(format!("{scope}_{status}"))
                .bind(scope)
                .bind(status)
                .bind(base)
                .execute(&pool)
                .await
                .unwrap();
            }
        }
        let mut tx = pool.begin().await.unwrap();
        refresh_upstream_resource_changes(&mut tx, "project", "prj_test")
            .await
            .unwrap();
        refresh_upstream_resource_changes(&mut tx, "org", "org_test")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let rows = sqlx::query("SELECT status, has_upstream_resource_changes FROM local_drafts")
            .fetch_all(&pool)
            .await
            .unwrap();
        for row in rows {
            let status: String = row.get("status");
            assert_eq!(
                row.get::<bool, _>("has_upstream_resource_changes"),
                matches!(status.as_str(), "open" | "submitted")
            );
        }
    }

    fn test_project_payload(version: i64, source: Option<i64>) -> ServerCommitPayload {
        let selection = json!({"project_id": "prj_test", "memories": [], "revision": 0});
        let content = format!("Memory version {version}");
        let selection_content = serde_json::to_string(&selection).unwrap();
        let blob = ServerBlob {
            blob_id: blob_object_id(&content),
            content,
        };
        let selection_blob = ServerBlob {
            blob_id: blob_object_id(&selection_content),
            content: selection_content,
        };
        ServerCommitPayload {
            commit: ServerCommit {
                commit_id: format!("commit_{version}"),
                scope: ServerCommitScope::Project,
                org_id: "org_test".to_owned(),
                project_id: Some("prj_test".to_owned()),
                tree_id: format!("tree_{version}"),
                parent_commit_id: (version > 1).then(|| format!("commit_{}", version - 1)),
                version,
                created_at: "2026-10-12T00:00:00Z".to_owned(),
            },
            tree: ServerTree {
                tree_id: format!("tree_{version}"),
                entries: vec![
                    ServerTreeEntry {
                        id: "memory_test".to_owned(),
                        kind: ServerTreeEntryKind::Memory,
                        scope: ServerTreeEntryScope::Project,
                        project_id: Some("prj_test".to_owned()),
                        path: Some("memory.md".to_owned()),
                        blob_id: blob.blob_id.clone(),
                        source: ServerTreeEntrySource::Project,
                        org_source: source.map(|source| crate::types::OrgMemorySource {
                            resource_id: "memory_org".to_owned(),
                            commit_id: format!("commit_{source}"),
                        }),
                        is_directory: false,
                        description: String::new(),
                    },
                    ServerTreeEntry {
                        id: "selection".to_owned(),
                        kind: ServerTreeEntryKind::ProjectOrgSelection,
                        scope: ServerTreeEntryScope::Daemon,
                        project_id: Some("prj_test".to_owned()),
                        path: None,
                        blob_id: selection_blob.blob_id.clone(),
                        source: ServerTreeEntrySource::Config,
                        org_source: None,
                        is_directory: false,
                        description: String::new(),
                    },
                ],
            },
            blobs: vec![blob, selection_blob],
            project_org_selection: Some(selection),
        }
    }

    struct NoCredentials;

    impl crate::CredentialStore for NoCredentials {
        fn load(&self) -> Result<Option<ServerCredentials>, crate::CredentialStoreError> {
            Ok(None)
        }
        fn replace(&self, _: &ServerCredentials) -> Result<(), crate::CredentialStoreError> {
            Ok(())
        }
        fn clear(&self) -> Result<(), crate::CredentialStoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn daemon_restart_waits_for_inflight_writer_and_recovers_unsynced_draft() {
        let root = tempfile::tempdir().unwrap();
        let config = crate::DaemonConfig::for_root(root.path());
        let state = DaemonState::initialize_with_credential_store(
            config.clone(),
            std::sync::Arc::new(NoCredentials),
        )
        .await
        .unwrap();
        let stored = state.store_draft_operation(serde_json::from_value(json!({
            "project_id": "prj_restart", "scope": "org", "resource": "memory", "source": "mcp_store",
            "op": {"create": {"path": "context/restart.md", "content": {"content": "unsynced body preserved"}}}
        })).unwrap()).await.unwrap();
        let mut writer = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        sqlx::query("UPDATE local_draft_operations SET sync_status = 'syncing' WHERE local_operation_id = $1")
            .bind(&stored.local_operation_id).execute(&mut *writer).await.unwrap();
        drop(state);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut restart = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            DaemonState::initialize_with_credential_store(
                config,
                std::sync::Arc::new(NoCredentials),
            )
            .await
        });
        started_rx.await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut restart)
                .await
                .is_err(),
            "restart must wait for the writer before reading the existing schema"
        );
        writer.commit().await.unwrap();
        let restarted = restart.await.unwrap().unwrap();
        let detail = restarted.get_draft(&stored.draft_id).await.unwrap();
        assert_eq!(
            detail.operations[0].sync_status,
            crate::types::DraftOperationSyncStatus::Queued
        );
        assert_eq!(detail.draft.pending_operation_count, 1);
        assert_eq!(
            detail.operations[0]
                .operation
                .create
                .as_ref()
                .unwrap()
                .content
                .content,
            "unsynced body preserved"
        );
    }

    #[tokio::test]
    async fn live_document_base_pin_survives_age_and_daemon_restart_then_releases() {
        let root = tempfile::tempdir().unwrap();
        let config = crate::DaemonConfig::for_root(root.path());
        let state = DaemonState::initialize_with_credential_store(
            config.clone(),
            std::sync::Arc::new(NoCredentials),
        )
        .await
        .unwrap();
        let payload = test_project_payload(1, None);
        let mut tx = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        cache_commit_payload(&mut tx, &payload).await.unwrap();
        tx.commit().await.unwrap();
        let pin = state
            .retain_commit_base(DaemonCommitBasePinRequest {
                commit_id: "commit_1".to_owned(),
                pin_id: None,
            })
            .await
            .unwrap();
        let acknowledgement = DaemonCommitBasePinRequest {
            commit_id: "commit_1".to_owned(),
            pin_id: Some(pin.pin_id.clone()),
        };
        assert!(
            state
                .retain_commit_base(acknowledgement.clone())
                .await
                .is_err()
        );
        let reader = std::fs::File::open(&pin.lock_path).unwrap();
        reader.try_lock_shared().unwrap();
        state
            .retain_commit_base(acknowledgement.clone())
            .await
            .unwrap();
        sqlx::query("UPDATE commit_base_pins SET issued_at = 0")
            .execute(&state.inner.pool)
            .await
            .unwrap();
        state.inner.pool.close().await;
        drop(state);
        let state = DaemonState::initialize_with_credential_store(
            config,
            std::sync::Arc::new(NoCredentials),
        )
        .await
        .unwrap();
        state.retain_commit_base(acknowledgement).await.unwrap();
        let mut tx = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        assert_eq!(
            live_commit_base_pins(&state, &mut tx).await.unwrap(),
            ["commit_1"]
        );
        tx.commit().await.unwrap();
        drop(reader);
        let mut tx = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        assert!(
            live_commit_base_pins(&state, &mut tx)
                .await
                .unwrap()
                .is_empty()
        );
        tx.commit().await.unwrap();
        cleanup_commit_base_pin_files(&state).await.unwrap();
        assert!(!Path::new(&pin.lock_path).exists());
    }

    #[tokio::test]
    async fn unclaimed_base_pin_expires_and_cannot_be_acknowledged_after_collection() {
        let root = tempfile::tempdir().unwrap();
        let state = DaemonState::initialize_with_credential_store(
            crate::DaemonConfig::for_root(root.path()),
            std::sync::Arc::new(NoCredentials),
        )
        .await
        .unwrap();
        let mut tx = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        cache_commit_payload(&mut tx, &test_project_payload(1, None))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let pin = state
            .retain_commit_base(DaemonCommitBasePinRequest {
                commit_id: "commit_1".to_owned(),
                pin_id: None,
            })
            .await
            .unwrap();
        let mut tx = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        assert_eq!(
            live_commit_base_pins(&state, &mut tx).await.unwrap(),
            ["commit_1"]
        );
        tx.commit().await.unwrap();
        sqlx::query("UPDATE commit_base_pins SET issued_at = 0")
            .execute(&state.inner.pool)
            .await
            .unwrap();
        let mut tx = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        assert!(
            live_commit_base_pins(&state, &mut tx)
                .await
                .unwrap()
                .is_empty()
        );
        tx.commit().await.unwrap();
        let reader = std::fs::File::open(&pin.lock_path).unwrap();
        reader.try_lock_shared().unwrap();
        let error = state
            .retain_commit_base(DaemonCommitBasePinRequest {
                commit_id: "commit_1".to_owned(),
                pin_id: Some(pin.pin_id),
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "commit_base_pin_expired",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn desktop_captured_base_preserves_old_and_empty_snapshots_after_head_advances() {
        let root = tempfile::tempdir().unwrap();
        let state = DaemonState::initialize_with_credential_store(
            crate::DaemonConfig::for_root(root.path()),
            std::sync::Arc::new(NoCredentials),
        )
        .await
        .unwrap();
        let mut tx = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        for version in [1, 2] {
            cache_commit_payload(&mut tx, &test_project_payload(version, None))
                .await
                .unwrap();
        }
        upsert_ref(
            &mut tx,
            &ServerRef {
                name: MAIN_REF.to_owned(),
                scope: ServerCommitScope::Project,
                org_id: "org_test".to_owned(),
                project_id: Some("prj_test".to_owned()),
                commit_id: Some("commit_2".to_owned()),
                updated_at: "now".to_owned(),
            },
            "head_2",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let service = DaemonIpcService::new(state.clone());
        let empty: DaemonDraftOperationResponse = service.dispatch(DaemonIpcRequest::new(
            "desktop_store_draft_operation", json!({
                "draft_id": null, "base_commit_id": null, "base_commit_captured": true,
                "project_id": "prj_test", "scope": "project", "resource": "memory", "source": "desktop",
                "op": {"create": {"path": "from-empty.md", "content": {"content": "An edit from the empty snapshot"}}}
            }),
        )).await.into_payload().unwrap();
        assert_eq!(
            service
                .get_draft(&empty.draft_id)
                .await
                .unwrap()
                .draft
                .base_commit_id,
            None
        );
        let old: DaemonDraftOperationResponse = service.dispatch(DaemonIpcRequest::new(
            "desktop_store_draft_operation", json!({
                "draft_id": null, "base_commit_id": "commit_1", "base_commit_captured": true,
                "project_id": "prj_test", "scope": "project", "resource": "memory", "source": "desktop",
                "op": {"update": {"id": "memory_test", "content": {"content": "Edit kept against the original Base"}}}
            }),
        )).await.into_payload().unwrap();
        assert_eq!(
            service
                .get_draft(&old.draft_id)
                .await
                .unwrap()
                .draft
                .base_commit_id
                .as_deref(),
            Some("commit_1")
        );
        let invalid = service.dispatch(DaemonIpcRequest::new(
            "desktop_store_draft_operation", json!({
                "draft_id": old.draft_id, "base_commit_id": null, "base_commit_captured": true,
                "project_id": "prj_test", "scope": "project", "resource": "memory", "source": "desktop",
                "op": {"update": {"id": "memory_test", "content": {"content": "Must not overwrite the existing Draft"}}}
            }),
        )).await.into_payload::<DaemonDraftOperationResponse>();
        assert!(invalid.is_err());
        assert_eq!(
            service
                .get_draft(&old.draft_id)
                .await
                .unwrap()
                .operations
                .len(),
            1
        );
        let batch: Vec<DaemonDraftOperationResponse> = service.dispatch(DaemonIpcRequest::new(
            "desktop_create_memory_drafts", json!({
                "project_id": "prj_test", "base_commit_id": null,
                "operations": [{"create": {"path": "empty-batch.md", "content": {"content": "Empty Base batch"}}}]
            }),
        )).await.into_payload().unwrap();
        assert_eq!(
            service
                .get_draft(&batch[0].draft_id)
                .await
                .unwrap()
                .draft
                .base_commit_id,
            None
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn base_pin_parent_symlink_cannot_release_a_live_document_or_delete_external_files() {
        let root = tempfile::tempdir().unwrap();
        let state = DaemonState::initialize_with_credential_store(
            crate::DaemonConfig::for_root(root.path()),
            std::sync::Arc::new(NoCredentials),
        )
        .await
        .unwrap();
        let mut tx = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        cache_commit_payload(&mut tx, &test_project_payload(1, None))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let pin = state
            .retain_commit_base(DaemonCommitBasePinRequest {
                commit_id: "commit_1".to_owned(),
                pin_id: None,
            })
            .await
            .unwrap();
        let reader = std::fs::File::open(&pin.lock_path).unwrap();
        reader.try_lock_shared().unwrap();
        state
            .retain_commit_base(DaemonCommitBasePinRequest {
                commit_id: "commit_1".to_owned(),
                pin_id: Some(pin.pin_id.clone()),
            })
            .await
            .unwrap();
        let directory = Path::new(&pin.lock_path).parent().unwrap();
        let old = directory.with_file_name("old-pins");
        std::fs::rename(directory, &old).unwrap();
        let outside = root.path().join("outside-pins");
        std::fs::create_dir(&outside).unwrap();
        let external_file = outside.join(format!("{}.lock", Uuid::new_v4()));
        std::fs::write(&external_file, b"do not delete").unwrap();
        std::os::unix::fs::symlink(&outside, directory).unwrap();
        let mut tx = state
            .inner
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        assert!(live_commit_base_pins(&state, &mut tx).await.is_err());
        tx.rollback().await.unwrap();
        assert!(cleanup_commit_base_pin_files(&state).await.is_err());
        assert_eq!(std::fs::read(external_file).unwrap(), b"do not delete");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM commit_base_pins")
                .fetch_one(&state.inner.pool)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn repeated_publications_prune_history_without_invalidating_active_readers_or_drafts() {
        let root = tempfile::tempdir().unwrap();
        let mut config = crate::DaemonConfig::for_root(root.path());
        config.project.server_url = "https://clumsies.test".to_owned();
        config.project.project_id = Some("prj_test".to_owned());
        let state = DaemonState::initialize_with_credential_store(
            config,
            std::sync::Arc::new(NoCredentials),
        )
        .await
        .unwrap();
        let mut pinned_reader = None;
        for version in 1..=4 {
            let payload = test_project_payload(version, None);
            let mut tx = state
                .inner
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .unwrap();
            cache_commit_payload(&mut tx, &payload).await.unwrap();
            tx.commit().await.unwrap();
            if version < 4 {
                let _sync = state.inner.sync_lock.lock().await;
                let reference = ServerRef {
                    name: MAIN_REF.to_owned(),
                    scope: ServerCommitScope::Project,
                    org_id: "org_test".to_owned(),
                    project_id: Some("prj_test".to_owned()),
                    commit_id: Some(payload.commit.commit_id.clone()),
                    updated_at: payload.commit.created_at.clone(),
                };
                let commit_state = ServerCommitState {
                    update_available: true,
                    reference,
                    latest: Some(payload.commit),
                    download_url: None,
                    incremental_supported: false,
                };
                install_ref(
                    &state,
                    &commit_state,
                    &format!("commit_{version}"),
                    Some("prj_test"),
                )
                .await
                .unwrap();
                if version == 1 {
                    sqlx::query("INSERT INTO local_drafts (draft_id, project_id, base_commit_id, current_commit_id, resource_scope, resource_kind, path, status) VALUES ('draft_test', 'prj_test', 'commit_1', 'commit_1', 'project', 'memory', 'proposal.md', 'open')")
                        .execute(&state.inner.pool).await.unwrap();
                }
            }
            if version == 2 {
                let pin = state
                    .retain_commit_base(DaemonCommitBasePinRequest {
                        commit_id: "commit_2".to_owned(),
                        pin_id: None,
                    })
                    .await
                    .unwrap();
                let reader = std::fs::File::open(&pin.lock_path).unwrap();
                reader.try_lock_shared().unwrap();
                state
                    .retain_commit_base(DaemonCommitBasePinRequest {
                        commit_id: "commit_2".to_owned(),
                        pin_id: Some(pin.pin_id),
                    })
                    .await
                    .unwrap();
                pinned_reader = Some(reader);
            }
        }
        let storage = super::super::project_storage::resolve_active(&state, "prj_test")
            .await
            .unwrap();
        let reader = state.inner.storage_access.read().await;
        let worker_state = state.clone();
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let worker_started = started.clone();
        let mut worker = tokio::spawn(async move {
            let _sync = worker_state.inner.sync_lock.lock().await;
            let payload = test_project_payload(4, None);
            let reference = ServerRef {
                name: MAIN_REF.to_owned(),
                scope: ServerCommitScope::Project,
                org_id: "org_test".to_owned(),
                project_id: Some("prj_test".to_owned()),
                commit_id: Some(payload.commit.commit_id.clone()),
                updated_at: payload.commit.created_at.clone(),
            };
            let commit_state = ServerCommitState {
                update_available: true,
                reference,
                latest: Some(payload.commit),
                download_url: None,
                incremental_supported: false,
            };
            worker_started.notify_one();
            install_ref(&worker_state, &commit_state, "commit_4", Some("prj_test"))
                .await
                .unwrap();
            install_ref(&worker_state, &commit_state, "commit_4", Some("prj_test"))
                .await
                .unwrap();
        });
        started.notified().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut worker)
                .await
                .is_err()
        );
        assert!(storage.generation_path("commit_2").exists());
        verify_generation_ref(
            &storage.generation_path("commit_3"),
            "prj_test",
            Some("commit_3"),
        )
        .unwrap();
        drop(reader);
        tokio::time::timeout(std::time::Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(storage.generation_path("commit_2").exists());
        verify_generation_ref(
            &storage.generation_path("commit_2"),
            "prj_test",
            Some("commit_2"),
        )
        .unwrap();
        for version in [1, 3, 4] {
            assert!(
                storage
                    .generation_path(&format!("commit_{version}"))
                    .exists()
            );
        }
        let remaining: Vec<String> =
            sqlx::query_scalar("SELECT commit_id FROM cached_commits ORDER BY commit_id")
                .fetch_all(&state.inner.pool)
                .await
                .unwrap();
        assert_eq!(
            remaining,
            vec!["commit_1", "commit_2", "commit_3", "commit_4"]
        );
        // The editor can retain a delivered Base without creating a Draft
        // until its first save, after several newer snapshots are installed.
        let saved = state
            .store_draft_operation(DaemonDraftOperationRequest {
                draft_id: None,
                base_commit_id: Some("commit_2".to_owned()),
                project_id: "prj_test".to_owned(),
                scope: DaemonDraftScope::Project,
                resource: DaemonDraftResourceKind::Memory,
                op: DaemonDraftOperation {
                    create: None,
                    update: Some(DaemonUpdateDraftOperation::Content(
                        DaemonContentDraftUpdate {
                            id: "memory_test".to_owned(),
                            content: DaemonDraftContent {
                                org_source: None,
                                is_directory: false,
                                description: None,
                                content: "First edit from an older open document".to_owned(),
                            },
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
        assert!(saved.queued);
        let effective = crate::search::load_effective_memory(&state, "prj_test")
            .await
            .unwrap();
        let edited = effective
            .resources
            .iter()
            .find(|resource| resource.resource_id == "memory_test")
            .unwrap();
        assert_eq!(edited.content, "First edit from an older open document");
        assert_eq!(edited.draft_id.as_deref(), Some(saved.draft_id.as_str()));

        drop(pinned_reader);
        let _sync = state.inner.sync_lock.lock().await;
        let _storage = state.inner.storage_access.write().await;
        sqlx::query("UPDATE local_drafts SET status = 'merged' WHERE draft_id = $1")
            .bind(&saved.draft_id)
            .execute(&state.inner.pool)
            .await
            .unwrap();
        prune_project_generations(&state, "prj_test", Some("commit_4"), Some("commit_3")).await;
        assert!(
            storage.generation_path("commit_2").exists(),
            "a terminal Draft with an unsynchronized operation must still retain its Base"
        );
        sqlx::query("UPDATE local_draft_operations SET sync_status = 'synced' WHERE draft_id = $1")
            .bind(&saved.draft_id)
            .execute(&state.inner.pool)
            .await
            .unwrap();
        prune_project_generations(&state, "prj_test", Some("commit_4"), Some("commit_3")).await;
        assert!(
            !storage.generation_path("commit_2").exists(),
            "released readers and fully synchronized terminal Drafts do not retain old generations"
        );
    }

    #[test]
    fn generations_retain_current_previous_and_draft_base_but_not_abandoned_work() {
        let root = tempfile::tempdir().unwrap();
        for version in 1..=4 {
            ensure_project_generation(
                root.path(),
                "prj_test",
                &test_project_payload(version, None),
                false,
            )
            .unwrap();
        }
        let generations = root.path().join("generations");
        let unknown = generations.join("user-data");
        std::fs::create_dir(&unknown).unwrap();
        std::fs::write(unknown.join("keep"), b"untouched").unwrap();
        let abandoned = generations.join(format!(".tmp-{}", Uuid::new_v4().simple()));
        std::fs::create_dir(&abandoned).unwrap();
        prune_generation_directories(
            root.path(),
            "prj_test",
            &BTreeSet::from([
                "commit_1".to_owned(),
                "commit_3".to_owned(),
                "commit_4".to_owned(),
            ]),
        )
        .unwrap();
        assert!(!generations.join("commit_2").exists());
        assert!(!abandoned.exists());
        for version in [1, 3, 4] {
            verify_generation_ref(
                &generations.join(format!("commit_{version}")),
                "prj_test",
                Some(&format!("commit_{version}")),
            )
            .unwrap();
        }
        assert_eq!(std::fs::read(unknown.join("keep")).unwrap(), b"untouched");
    }

    #[test]
    fn corrupt_generation_is_only_replaced_after_a_valid_candidate_is_ready() {
        let root = tempfile::tempdir().unwrap();
        let generation = ensure_empty_generation(root.path(), "prj_test", false).unwrap();
        std::fs::write(generation.join("manifest.json"), b"corrupt").unwrap();
        let marker =
            serde_json::to_vec(&json!({"project_id": "prj_test", "commit_id": null})).unwrap();
        assert!(
            ensure_generation(
                root.path(),
                "prj_test",
                EMPTY_GENERATION,
                &marker,
                true,
                |_| { Err(DaemonError::Server("materialization failed".to_owned())) }
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(generation.join("manifest.json")).unwrap(),
            b"corrupt"
        );
        assert!(ensure_empty_generation(root.path(), "prj_test", false).is_err());
        ensure_empty_generation(root.path(), "prj_test", true).unwrap();
        verify_generation_ref(&generation, "prj_test", None).unwrap();
        assert_eq!(
            std::fs::read_dir(root.path().join("generations"))
                .unwrap()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn cached_commit_repairs_scalar_projection_without_overwriting_valid_history() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool).await.unwrap();
        let original = test_project_payload(1, None);
        let mut tx = pool.begin().await.unwrap();
        cache_commit_payload(&mut tx, &original).await.unwrap();
        tx.commit().await.unwrap();
        for corrupt_json in [false, true] {
            sqlx::query(
                "UPDATE cached_commits SET scope = 'org', org_id = 'wrong-org',
                project_id = NULL, tree_id = 'wrong-tree', parent_commit_id = 'wrong-parent',
                version = 99, created_at = 'wrong-time' WHERE commit_id = 'commit_1'",
            )
            .execute(&pool)
            .await
            .unwrap();
            if corrupt_json {
                sqlx::query(
                    "UPDATE cached_commits SET payload_json = '{}' WHERE commit_id = 'commit_1'",
                )
                .execute(&pool)
                .await
                .unwrap();
            }
            assert!(
                load_cached_commit_payload(&pool, "commit_1")
                    .await
                    .unwrap()
                    .is_none()
            );
            let mut tx = pool.begin().await.unwrap();
            cache_commit_payload(&mut tx, &original).await.unwrap();
            tx.commit().await.unwrap();
            let record: CachedCommitRecord =
                sqlx::query_as("SELECT * FROM cached_commits WHERE commit_id = 'commit_1'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(record.matches(&original.commit));
            let restored = load_cached_commit_payload(&pool, "commit_1")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(restored.commit, original.commit);
            assert_eq!(restored.tree, original.tree);
            let mut tx = pool.begin().await.unwrap();
            assert!(
                cached_resource_entry(&mut tx, Some("commit_1"), "memory_test", "memory")
                    .await
                    .unwrap()
                    .is_some()
            );
            tx.rollback().await.unwrap();
        }
        let mut changed = original.clone();
        changed.commit.created_at = "different-valid-time".to_owned();
        let mut tx = pool.begin().await.unwrap();
        let error = cache_commit_payload(&mut tx, &changed).await.unwrap_err();
        assert!(error.to_string().contains("violates immutability"));
        tx.rollback().await.unwrap();
        assert_eq!(
            load_cached_commit_payload(&pool, "commit_1")
                .await
                .unwrap()
                .unwrap()
                .commit,
            original.commit
        );
    }

    #[tokio::test]
    async fn canonical_memory_draft_reads_legacy_immutable_tree_kind() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool).await.unwrap();
        for kind in [
            ServerTreeEntryKind::Context,
            ServerTreeEntryKind::Rule,
            ServerTreeEntryKind::Workflow,
        ] {
            let mut payload = test_project_payload(1, None);
            payload.tree.entries[0].kind = kind;
            sqlx::query("DELETE FROM cached_commits")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM cached_trees")
                .execute(&pool)
                .await
                .unwrap();
            let mut tx = pool.begin().await.unwrap();
            cache_commit_payload(&mut tx, &payload).await.unwrap();
            let entry = cached_resource_entry(&mut tx, Some("commit_1"), "memory_test", "memory")
                .await
                .unwrap();
            assert_eq!(
                entry,
                Some((
                    Some("memory.md".into()),
                    payload.tree.entries[0].blob_id.clone()
                ))
            );
            tx.commit().await.unwrap();
        }
    }

    #[tokio::test]
    async fn valid_cached_tree_still_rejects_an_immutable_collision() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool).await.unwrap();
        let mut original = test_project_payload(1, None);
        let mut tx = pool.begin().await.unwrap();
        cache_commit_payload(&mut tx, &original).await.unwrap();
        tx.commit().await.unwrap();
        let mut changed = original.clone();
        changed.tree.entries[0].path = Some("different.md".to_owned());
        validate_commit_objects(&changed).unwrap();
        let mut tx = pool.begin().await.unwrap();
        let error = cache_commit_payload(&mut tx, &changed).await.unwrap_err();
        assert!(error.to_string().contains("violates immutability"));
        tx.rollback().await.unwrap();
        let mut cached = load_cached_commit_payload(&pool, "commit_1")
            .await
            .unwrap()
            .unwrap();
        original.blobs.sort_by(|a, b| a.blob_id.cmp(&b.blob_id));
        cached.blobs.sort_by(|a, b| a.blob_id.cmp(&b.blob_id));
        assert_eq!(cached, original);
    }

    #[tokio::test]
    async fn cached_tree_collisions_preserve_prior_ownership_even_without_valid_blob_bytes() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool).await.unwrap();
        let original = test_project_payload(1, None);
        let expected_tree = serde_json::to_string(&original.tree).unwrap();
        let mut other_project = test_project_payload(2, None);
        other_project.commit.project_id = Some("prj_other".to_owned());
        other_project.commit.tree_id = original.tree.tree_id.clone();
        other_project.tree.tree_id = original.tree.tree_id.clone();
        for entry in &mut other_project.tree.entries {
            entry.project_id = Some("prj_other".to_owned());
        }
        let selection = other_project.project_org_selection.as_mut().unwrap();
        selection["project_id"] = json!("prj_other");
        let selection_content = serde_json::to_string(selection).unwrap();
        other_project.blobs[1] = ServerBlob {
            blob_id: blob_object_id(&selection_content),
            content: selection_content,
        };
        other_project.tree.entries[1].blob_id = other_project.blobs[1].blob_id.clone();
        let mut organization = test_project_payload(2, None);
        organization.commit.scope = ServerCommitScope::Org;
        organization.commit.project_id = None;
        organization.commit.tree_id = original.tree.tree_id.clone();
        organization.tree.tree_id = original.tree.tree_id.clone();
        organization.tree.entries.truncate(1);
        organization.tree.entries[0].scope = ServerTreeEntryScope::Org;
        organization.tree.entries[0].source = ServerTreeEntrySource::Org;
        organization.tree.entries[0].project_id = None;
        organization.blobs.truncate(1);
        organization.project_org_selection = None;
        for incoming in [other_project, organization] {
            validate_commit_objects(&incoming).unwrap();
            for corruption in ["complete", "missing_blob", "corrupt_blob", "scalar_owner"] {
                let mut tx = pool.begin().await.unwrap();
                cache_commit_payload(&mut tx, &original).await.unwrap();
                tx.commit().await.unwrap();
                match corruption {
                    "scalar_owner" => {
                        sqlx::query("UPDATE cached_commits SET scope = 'org', project_id = NULL WHERE commit_id = $1")
                            .bind(&original.commit.commit_id).execute(&pool).await.unwrap();
                    }
                    "missing_blob" => {
                        sqlx::query("DELETE FROM cached_blobs WHERE blob_id = $1")
                            .bind(&original.blobs[0].blob_id)
                            .execute(&pool)
                            .await
                            .unwrap();
                    }
                    "corrupt_blob" => {
                        sqlx::query(
                            "UPDATE cached_blobs SET content = 'damaged' WHERE blob_id = $1",
                        )
                        .bind(&original.blobs[0].blob_id)
                        .execute(&pool)
                        .await
                        .unwrap();
                    }
                    _ => {}
                }
                let mut tx = pool.begin().await.unwrap();
                let error = cache_commit_payload(&mut tx, &incoming).await.unwrap_err();
                assert!(
                    error.to_string().contains("Cached Tree"),
                    "{corruption}: {error}"
                );
                tx.rollback().await.unwrap();
                let tree: String =
                    sqlx::query_scalar("SELECT payload_json FROM cached_trees WHERE tree_id = $1")
                        .bind(&original.tree.tree_id)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(tree, expected_tree);
                let changed_commit: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM cached_commits WHERE commit_id = $1")
                        .bind(&incoming.commit.commit_id)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(changed_commit, 0);
                let new_blob: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM cached_blobs WHERE blob_id = $1")
                        .bind(&incoming.blobs[0].blob_id)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(
                    new_blob, 0,
                    "rejecting a collision rolls back newly inserted Blob bytes"
                );
            }
        }
        sqlx::query("UPDATE cached_trees SET payload_json = 'malformed' WHERE tree_id = $1")
            .bind(&original.tree.tree_id)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        cache_commit_payload(&mut tx, &original).await.unwrap();
        tx.commit().await.unwrap();
        assert!(
            load_cached_commit_payload(&pool, &original.commit.commit_id)
                .await
                .unwrap()
                .is_some()
        );
    }

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
                org_source: None,
                is_directory: false,
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
                org_source: None,
                is_directory: false,
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
