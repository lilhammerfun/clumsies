use std::ffi::CString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use super::{DaemonError, DaemonState, canonical_server_url};

const STORAGE_LAYOUT_VERSION: u32 = 1;
const STORAGE_DIRECTORY: &str = ".clumsies/cache-v1";
const DEFAULT_LOCATION_REVISION: i64 = 1;
const SEARCH_BUILD_RESERVE_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(target_os = "macos")]
type SecurityScope = Option<Arc<SecurityScopedAccess>>;

#[cfg(not(target_os = "macos"))]
type SecurityScope = ();

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct SecurityScopedAccess {
    url: objc2::rc::Retained<objc2_foundation::NSURL>,
}

#[cfg(target_os = "macos")]
impl Drop for SecurityScopedAccess {
    fn drop(&mut self) {
        unsafe { self.url.stopAccessingSecurityScopedResource() };
    }
}

#[derive(Debug)]
struct ResolvedSelectedRoot {
    path: PathBuf,
    bookmark_to_persist: Option<String>,
    _security_scope: SecurityScope,
}

/// Failure classification for a persisted macOS authorization bookmark.
enum BookmarkFailure {
    /// The bookmark cannot be repaired from the selected path; the error is
    /// reported to the caller as-is.
    Fatal(DaemonError),
    /// The bookmark references a file identity that no longer exists; the
    /// persisted selected path may still be reachable and re-authorizable.
    Stale(DaemonError),
}

impl From<DaemonError> for BookmarkFailure {
    fn from(error: DaemonError) -> Self {
        BookmarkFailure::Fatal(error)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonProjectStorageMode {
    Default,
    Custom,
}

impl DaemonProjectStorageMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Custom => "custom",
        }
    }

    fn parse(value: &str) -> Result<Self, DaemonError> {
        match value {
            "default" => Ok(Self::Default),
            "custom" => Ok(Self::Custom),
            _ => Err(DaemonError::State {
                code: "project_storage_corrupt",
                message: format!("Unknown Project storage mode: {value}"),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonProjectStorageAvailability {
    Ready,
    Moving,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonProjectStorageMoveState {
    Preparing,
    Materializing,
    Verifying,
    Switching,
    Cleaning,
    Completed,
    Failed,
}

impl DaemonProjectStorageMoveState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Materializing => "materializing",
            Self::Verifying => "verifying",
            Self::Switching => "switching",
            Self::Cleaning => "cleaning",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, DaemonError> {
        match value {
            "preparing" => Ok(Self::Preparing),
            "materializing" => Ok(Self::Materializing),
            "verifying" => Ok(Self::Verifying),
            "switching" => Ok(Self::Switching),
            "cleaning" => Ok(Self::Cleaning),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(DaemonError::State {
                code: "project_storage_corrupt",
                message: format!("Unknown Project storage move state: {value}"),
            }),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectStorageRequest {
    pub project_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectStorageMoveRequest {
    pub move_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectStorageReplaceRequest {
    pub project_id: String,
    pub selected_root_path: String,
    pub handoff_bookmark_data: String,
    pub expected_location_revision: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectStorageResetRequest {
    pub project_id: String,
    pub expected_location_revision: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectCacheClearRequest {
    pub project_id: String,
    pub expected_location_revision: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectStorage {
    pub authority_key: String,
    pub project_id: String,
    pub mode: DaemonProjectStorageMode,
    pub selected_root_path: String,
    pub managed_root_path: String,
    pub active_generation_path: Option<String>,
    pub search_index_path: String,
    pub availability: DaemonProjectStorageAvailability,
    pub location_revision: i64,
    pub size_bytes: u64,
    pub active_move_id: Option<String>,
    pub issue_code: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectStorageMove {
    pub move_id: String,
    pub project_id: String,
    pub source_mode: DaemonProjectStorageMode,
    pub destination_mode: DaemonProjectStorageMode,
    pub source_managed_root_path: String,
    pub destination_managed_root_path: String,
    pub source_location_revision: i64,
    pub state: DaemonProjectStorageMoveState,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveProjectStorage {
    pub(crate) authority_key: String,
    pub(crate) project_id: String,
    pub(crate) mode: DaemonProjectStorageMode,
    pub(crate) selected_root: PathBuf,
    pub(crate) managed_root: PathBuf,
    pub(crate) location_revision: i64,
    _security_scope: SecurityScope,
}

impl ActiveProjectStorage {
    pub(crate) fn generation_path(&self, generation: &str) -> PathBuf {
        self.managed_root.join("generations").join(generation)
    }

    pub(crate) fn search_index_path(&self) -> PathBuf {
        self.managed_root.join("search/index.sqlite")
    }
}

#[derive(Clone, Debug)]
struct StorageSetting {
    mode: DaemonProjectStorageMode,
    selected_root_path: Option<String>,
    bookmark_data: Option<String>,
    location_revision: i64,
}

#[derive(Clone, Debug)]
struct StorageMoveRow {
    move_id: String,
    authority_key: String,
    project_id: String,
    source_mode: DaemonProjectStorageMode,
    source_selected_root_path: String,
    source_bookmark_data: Option<String>,
    source_managed_root_path: String,
    source_location_revision: i64,
    destination_mode: DaemonProjectStorageMode,
    destination_selected_root_path: String,
    destination_bookmark_data: Option<String>,
    destination_managed_root_path: String,
    state: DaemonProjectStorageMoveState,
}

#[derive(Deserialize, Serialize)]
struct OwnershipMarker {
    layout_version: u32,
    authority_key: String,
    project_id: String,
}

pub(super) async fn migrate(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS project_storage_locations (
            authority_key TEXT NOT NULL,
            project_id TEXT NOT NULL,
            mode TEXT NOT NULL CHECK (mode IN ('default', 'custom')),
            selected_root_path TEXT,
            bookmark_data TEXT,
            location_revision BIGINT NOT NULL CHECK (location_revision > 0),
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (authority_key, project_id),
            CHECK (
                (mode = 'default' AND selected_root_path IS NULL AND bookmark_data IS NULL)
                OR
                (mode = 'custom' AND selected_root_path IS NOT NULL AND bookmark_data IS NOT NULL)
            )
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS project_storage_moves (
            move_id TEXT PRIMARY KEY,
            authority_key TEXT NOT NULL,
            project_id TEXT NOT NULL,
            source_mode TEXT NOT NULL CHECK (source_mode IN ('default', 'custom')),
            source_selected_root_path TEXT NOT NULL,
            source_bookmark_data TEXT,
            source_managed_root_path TEXT NOT NULL,
            source_location_revision BIGINT NOT NULL CHECK (source_location_revision > 0),
            destination_mode TEXT NOT NULL CHECK (destination_mode IN ('default', 'custom')),
            destination_selected_root_path TEXT NOT NULL,
            destination_bookmark_data TEXT,
            destination_managed_root_path TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN (
                'preparing', 'materializing', 'verifying', 'switching',
                'cleaning', 'completed', 'failed'
            )),
            error_code TEXT,
            error_message TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            completed_at TEXT
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_project_storage_moves_active
         ON project_storage_moves (authority_key, project_id)
         WHERE state NOT IN ('completed', 'failed')",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn resolve_active(
    state: &DaemonState,
    project_id: &str,
) -> Result<ActiveProjectStorage, DaemonError> {
    super::commit_sync::validate_cache_component("project_id", project_id)?;
    let authority_key = authority_key(state)?;
    let setting = load_setting(&state.inner.pool, &authority_key, project_id).await?;
    resolve_setting(state, &authority_key, project_id, setting).await
}

pub(super) async fn project_storage(
    state: &DaemonState,
    request: DaemonProjectStorageRequest,
) -> Result<DaemonProjectStorage, DaemonError> {
    super::commit_sync::validate_cache_component("project_id", &request.project_id)?;
    let _storage = state.inner.storage_access.read().await;
    let authority_key = authority_key(state)?;
    let setting = load_setting(&state.inner.pool, &authority_key, &request.project_id).await?;
    let projection =
        projected_storage(state, &authority_key, &request.project_id, setting.as_ref())?;
    let active_move_id =
        active_move_id(&state.inner.pool, &authority_key, &request.project_id).await?;
    let completed_move_issue = if active_move_id.is_none() {
        latest_completed_move_issue(&state.inner.pool, &authority_key, &request.project_id).await?
    } else {
        None
    };

    match resolve_setting(state, &authority_key, &request.project_id, setting).await {
        Ok(active) => {
            let size_root = active.managed_root.clone();
            let size_bytes = tokio::task::spawn_blocking(move || managed_size(&size_root))
                .await
                .map_err(|error| {
                    storage_error(
                        "storage_verification_failed",
                        format!("Project storage size task failed: {error}"),
                    )
                })??;
            let active_generation_path = active_generation_path(state, &active).await?;
            Ok(DaemonProjectStorage {
                authority_key,
                project_id: request.project_id,
                mode: active.mode,
                selected_root_path: active.selected_root.display().to_string(),
                managed_root_path: active.managed_root.display().to_string(),
                active_generation_path,
                search_index_path: active.search_index_path().display().to_string(),
                availability: if active_move_id.is_some() {
                    DaemonProjectStorageAvailability::Moving
                } else {
                    DaemonProjectStorageAvailability::Ready
                },
                location_revision: active.location_revision,
                size_bytes,
                active_move_id,
                issue_code: completed_move_issue.as_ref().map(|issue| issue.0.clone()),
                diagnostic: completed_move_issue.map(|issue| issue.1),
            })
        }
        Err(error) => {
            let (issue_code, diagnostic) = storage_error_parts(&error);
            Ok(DaemonProjectStorage {
                authority_key,
                project_id: request.project_id,
                mode: projection.mode,
                selected_root_path: projection.selected_root.display().to_string(),
                managed_root_path: projection.managed_root.display().to_string(),
                active_generation_path: None,
                search_index_path: projection.search_index_path().display().to_string(),
                availability: DaemonProjectStorageAvailability::Unavailable,
                location_revision: projection.location_revision,
                size_bytes: 0,
                active_move_id,
                issue_code: Some(issue_code),
                diagnostic: Some(diagnostic),
            })
        }
    }
}

pub(super) async fn replace_project_storage(
    state: &DaemonState,
    request: DaemonProjectStorageReplaceRequest,
) -> Result<DaemonProjectStorageMove, DaemonError> {
    super::commit_sync::validate_cache_component("project_id", &request.project_id)?;
    if request.expected_location_revision < 1 {
        return Err(DaemonError::InvalidRequest(
            "expected_location_revision must be positive".to_owned(),
        ));
    }
    let authority_key = authority_key(state)?;
    let setting = load_setting(&state.inner.pool, &authority_key, &request.project_id).await?;
    let projection =
        projected_storage(state, &authority_key, &request.project_id, setting.as_ref())?;
    ensure_expected_revision(
        request.expected_location_revision,
        projection.location_revision,
    )?;

    let selected =
        resolve_handoff_selected_root(&request.selected_root_path, &request.handoff_bookmark_data)?;
    let selected_root = &selected.path;
    let destination_root = custom_managed_root(selected_root, &authority_key, &request.project_id);
    let destination_bookmark = selected.bookmark_to_persist.as_deref().ok_or_else(|| {
        storage_error(
            "permission_required",
            "The Project storage handoff did not produce daemon authorization",
        )
    })?;
    if projection.mode == DaemonProjectStorageMode::Custom
        && paths_equivalent(&destination_root, &projection.managed_root)
    {
        return refresh_custom_authorization(
            state,
            &projection,
            setting
                .as_ref()
                .and_then(|setting| setting.bookmark_data.as_deref()),
            selected_root,
            destination_bookmark,
        )
        .await;
    }

    let current = resolve_setting(state, &authority_key, &request.project_id, setting).await?;
    validate_custom_selected_root(state, selected_root, &current.managed_root)?;
    create_move(
        state,
        &current,
        DaemonProjectStorageMode::Custom,
        selected_root,
        Some(destination_bookmark),
        &destination_root,
    )
    .await
}

async fn refresh_custom_authorization(
    state: &DaemonState,
    current: &ActiveProjectStorage,
    source_bookmark: Option<&str>,
    selected_root: &Path,
    destination_bookmark: &str,
) -> Result<DaemonProjectStorageMove, DaemonError> {
    validate_managed_path_components(selected_root, &current.managed_root)?;
    verify_ownership_marker(
        &current.managed_root,
        &current.authority_key,
        &current.project_id,
    )?;

    let move_id = format!("move_{}", Uuid::new_v4().simple());
    let mut tx = state.inner.pool.begin().await?;
    sqlx::query(
        "INSERT INTO project_storage_moves (
            move_id, authority_key, project_id,
            source_mode, source_selected_root_path, source_bookmark_data,
            source_managed_root_path, source_location_revision,
            destination_mode, destination_selected_root_path,
            destination_bookmark_data, destination_managed_root_path, state
         ) VALUES ($1, $2, $3, 'custom', $4, $5, $6, $7,
                   'custom', $8, $9, $10, 'switching')",
    )
    .bind(&move_id)
    .bind(&current.authority_key)
    .bind(&current.project_id)
    .bind(current.selected_root.display().to_string())
    .bind(source_bookmark)
    .bind(current.managed_root.display().to_string())
    .bind(current.location_revision)
    .bind(selected_root.display().to_string())
    .bind(destination_bookmark)
    .bind(current.managed_root.display().to_string())
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        if error
            .to_string()
            .contains("idx_project_storage_moves_active")
        {
            storage_error(
                "storage_move_conflict",
                "A Project storage move is already in progress",
            )
        } else {
            error.into()
        }
    })?;

    let next_revision = current.location_revision + 1;
    let updated = sqlx::query(
        "UPDATE project_storage_locations
         SET selected_root_path = $4, bookmark_data = $5, location_revision = $6,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE authority_key = $1 AND project_id = $2
           AND mode = 'custom' AND location_revision = $3",
    )
    .bind(&current.authority_key)
    .bind(&current.project_id)
    .bind(current.location_revision)
    .bind(selected_root.display().to_string())
    .bind(destination_bookmark)
    .bind(next_revision)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(storage_error(
            "storage_move_conflict",
            "Project storage location changed before its authorization could be refreshed",
        ));
    }

    sqlx::query(
        "UPDATE project_storage_moves
         SET state = 'completed', completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE move_id = $1",
    )
    .bind(&move_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    load_move_view(&state.inner.pool, &move_id).await
}

pub(super) async fn reset_project_storage(
    state: &DaemonState,
    request: DaemonProjectStorageResetRequest,
) -> Result<DaemonProjectStorageMove, DaemonError> {
    super::commit_sync::validate_cache_component("project_id", &request.project_id)?;
    let current = resolve_active(state, &request.project_id).await?;
    ensure_expected_revision(
        request.expected_location_revision,
        current.location_revision,
    )?;
    if current.mode == DaemonProjectStorageMode::Default {
        return Err(DaemonError::InvalidRequest(
            "the Project already uses the default storage location".to_owned(),
        ));
    }
    let selected_root = default_selected_root(state)?;
    let destination_root =
        default_managed_root(&selected_root, &current.authority_key, &request.project_id);
    create_move(
        state,
        &current,
        DaemonProjectStorageMode::Default,
        &selected_root,
        None,
        &destination_root,
    )
    .await
}

pub(super) async fn project_storage_move(
    state: &DaemonState,
    request: DaemonProjectStorageMoveRequest,
) -> Result<DaemonProjectStorageMove, DaemonError> {
    load_move_view(&state.inner.pool, &request.move_id).await
}

pub(super) async fn clear_project_cache(
    state: &DaemonState,
    request: DaemonProjectCacheClearRequest,
) -> Result<DaemonProjectStorage, DaemonError> {
    super::commit_sync::validate_cache_component("project_id", &request.project_id)?;
    let _sync = state.inner.sync_lock.lock().await;
    let _search = state.inner.search_lock.lock().await;
    let _storage = state.inner.storage_access.write().await;
    let active = resolve_active(state, &request.project_id).await?;
    ensure_expected_revision(request.expected_location_revision, active.location_revision)?;
    verify_ownership_marker(
        &active.managed_root,
        &active.authority_key,
        &active.project_id,
    )?;

    let managed_root = active.managed_root.clone();
    tokio::task::spawn_blocking(move || {
        for child in ["generations", "search", "staging"] {
            remove_if_exists(&managed_root.join(child))?;
        }
        ensure_private_directory(&managed_root.join("generations"))?;
        ensure_private_directory(&managed_root.join("search"))?;
        ensure_private_directory(&managed_root.join("staging"))?;
        Ok::<(), DaemonError>(())
    })
    .await
    .map_err(|error| {
        storage_error(
            "storage_verification_failed",
            format!("Project cache clear task failed: {error}"),
        )
    })??;

    let mut tx = state.inner.pool.begin().await?;
    sqlx::query("DELETE FROM cached_refs WHERE scope = 'project' AND project_id = $1")
        .bind(&request.project_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM search_heads WHERE project_id = $1")
        .bind(&request.project_id)
        .execute(&mut *tx)
        .await?;
    // Invalidate any build that loaded the Project before this destructive
    // clear. Its publish CAS must observe a newer desired sequence and refuse
    // to restore the removed search head.
    super::search::scheduler::enqueue_project_in_tx(&mut tx, &request.project_id).await?;
    tx.commit().await?;
    state.inner.search_index_notify.notify_one();
    drop(_storage);
    drop(_search);
    drop(_sync);
    state.request_sync();
    project_storage(
        state,
        DaemonProjectStorageRequest {
            project_id: request.project_id,
        },
    )
    .await
}

pub(super) fn resume_pending_moves(state: &DaemonState) {
    let state = state.clone();
    tokio::spawn(async move {
        let move_ids = sqlx::query_scalar::<_, String>(
            "SELECT move_id FROM project_storage_moves
             WHERE state NOT IN ('completed', 'failed')
             ORDER BY created_at",
        )
        .fetch_all(&state.inner.pool)
        .await;
        let Ok(move_ids) = move_ids else {
            return;
        };
        for move_id in move_ids {
            spawn_move_worker(state.clone(), move_id);
        }
    });
}

pub(crate) fn ensure_managed_root(
    root: &Path,
    authority_key: &str,
    project_id: &str,
) -> Result<(), DaemonError> {
    let mut repair_legacy_permissions = false;
    let mut marker_exists = false;
    if root.exists() {
        if !root.is_dir() {
            return Err(storage_error(
                "marker_mismatch",
                format!("Managed storage path {} is not a directory", root.display()),
            ));
        }
        let marker_path = root.join("ownership.json");
        if marker_path.exists() {
            verify_ownership_marker(root, authority_key, project_id)?;
            marker_exists = true;
        } else if fs::read_dir(root)?.next().is_some() && !is_legacy_default_root(root) {
            return Err(storage_error(
                "marker_mismatch",
                format!(
                    "Managed storage path {} contains data without a Clumsies ownership marker",
                    root.display()
                ),
            ));
        } else if is_legacy_default_root(root) {
            repair_legacy_permissions = true;
        }
    }
    ensure_private_directory(root)?;
    if !marker_exists {
        let marker = OwnershipMarker {
            layout_version: STORAGE_LAYOUT_VERSION,
            authority_key: authority_key.to_owned(),
            project_id: project_id.to_owned(),
        };
        write_private_file(
            &root.join("ownership.json"),
            &serde_json::to_vec_pretty(&marker)?,
        )?;
    }
    for child in ["generations", "search", "staging"] {
        ensure_private_directory(&root.join(child))?;
    }
    if repair_legacy_permissions {
        secure_managed_tree(root)?;
    }
    Ok(())
}

pub(crate) fn ensure_private_directory(path: &Path) -> Result<(), DaemonError> {
    fs::create_dir_all(path)?;
    set_mode(path, 0o700)?;
    Ok(())
}

pub(crate) fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), DaemonError> {
    if let Some(parent) = path.parent() {
        ensure_private_directory(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            DaemonError::InvalidRequest("Managed file name is not valid UTF-8".to_owned())
        })?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        set_mode(&temporary, 0o600)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok::<(), DaemonError>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(())
}

pub(crate) fn secure_managed_tree(root: &Path) -> Result<(), DaemonError> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(storage_error(
            "marker_mismatch",
            format!(
                "Managed storage contains a symbolic link: {}",
                root.display()
            ),
        ));
    }
    if metadata.is_dir() {
        set_mode_allowing_disappearance(root, 0o700)?;
        for entry in fs::read_dir(root)? {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            secure_managed_tree(&path)?;
        }
    } else if metadata.is_file() {
        set_mode_allowing_disappearance(root, 0o600)?;
    }
    Ok(())
}

fn set_mode_allowing_disappearance(path: &Path, mode: u32) -> Result<(), DaemonError> {
    match set_mode(path, mode) {
        Err(DaemonError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

async fn create_move(
    state: &DaemonState,
    current: &ActiveProjectStorage,
    destination_mode: DaemonProjectStorageMode,
    destination_selected_root: &Path,
    destination_bookmark: Option<&str>,
    destination_managed_root: &Path,
) -> Result<DaemonProjectStorageMove, DaemonError> {
    if active_move_id(
        &state.inner.pool,
        &current.authority_key,
        &current.project_id,
    )
    .await?
    .is_some()
    {
        return Err(storage_error(
            "storage_move_conflict",
            "A Project storage move is already in progress",
        ));
    }
    let setting = load_setting(
        &state.inner.pool,
        &current.authority_key,
        &current.project_id,
    )
    .await?;
    let source_bookmark = setting.and_then(|setting| setting.bookmark_data);
    let move_id = format!("move_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO project_storage_moves (
            move_id, authority_key, project_id,
            source_mode, source_selected_root_path, source_bookmark_data,
            source_managed_root_path, source_location_revision,
            destination_mode, destination_selected_root_path,
            destination_bookmark_data, destination_managed_root_path, state
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'preparing')",
    )
    .bind(&move_id)
    .bind(&current.authority_key)
    .bind(&current.project_id)
    .bind(current.mode.as_str())
    .bind(current.selected_root.display().to_string())
    .bind(source_bookmark)
    .bind(current.managed_root.display().to_string())
    .bind(current.location_revision)
    .bind(destination_mode.as_str())
    .bind(destination_selected_root.display().to_string())
    .bind(destination_bookmark)
    .bind(destination_managed_root.display().to_string())
    .execute(&state.inner.pool)
    .await
    .map_err(|error| {
        if error
            .to_string()
            .contains("idx_project_storage_moves_active")
        {
            storage_error(
                "storage_move_conflict",
                "A Project storage move is already in progress",
            )
        } else {
            error.into()
        }
    })?;
    spawn_move_worker(state.clone(), move_id.clone());
    load_move_view(&state.inner.pool, &move_id).await
}

fn spawn_move_worker(state: DaemonState, move_id: String) {
    tokio::spawn(async move {
        if let Err(error) = run_move(&state, &move_id).await {
            let (code, message) = storage_error_parts(&error);
            let phase = sqlx::query_scalar::<_, String>(
                "SELECT state FROM project_storage_moves WHERE move_id = $1",
            )
            .bind(&move_id)
            .fetch_optional(&state.inner.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "unknown".to_owned());
            let _ = sqlx::query(
                "UPDATE project_storage_moves
                 SET state = 'failed', error_code = $2, error_message = $3,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE move_id = $1 AND state NOT IN ('completed', 'failed')",
            )
            .bind(&move_id)
            .bind(code)
            .bind(format!(
                "Project storage move failed during {phase}: {message}"
            ))
            .execute(&state.inner.pool)
            .await;
        }
    });
}

async fn run_move(state: &DaemonState, move_id: &str) -> Result<(), DaemonError> {
    let _sync = state.inner.sync_lock.lock().await;
    let move_row = load_move_row(&state.inner.pool, move_id).await?;
    if move_row.state.is_terminal() {
        return Ok(());
    }

    if move_row.state == DaemonProjectStorageMoveState::Cleaning {
        let _storage = state.inner.storage_access.write().await;
        // `switch_location` commits the active location and the cleaning
        // phase together. A crash can therefore happen before the normal
        // path mirrors the copied project-index head and schedules a build.
        // Reconcile both durable search records before deleting the source so
        // restart recovery cannot leave the central head on the old location.
        super::search::publish_project_index_head(state, &move_row.project_id).await?;
        super::search::scheduler::enqueue_project(state, &move_row.project_id).await?;
        let _security_scopes = match validate_move_paths(&move_row, false) {
            Ok(scopes) => scopes,
            Err(error) => {
                let (_, message) = storage_error_parts(&error);
                complete_move_with_cleanup_warning(&state.inner.pool, move_id, message).await?;
                return Ok(());
            }
        };
        finish_cleanup(&state.inner.pool, move_id, &move_row).await?;
        return Ok(());
    }

    let _security_scopes = validate_move_paths(&move_row, true)?;
    update_move_state(
        &state.inner.pool,
        move_id,
        DaemonProjectStorageMoveState::Preparing,
    )
    .await?;
    let source_bytes = managed_size(Path::new(&move_row.source_managed_root_path))?;
    let required_bytes = source_bytes
        .saturating_mul(2)
        .saturating_add(SEARCH_BUILD_RESERVE_BYTES);
    ensure_available_space(
        Path::new(&move_row.destination_selected_root_path),
        required_bytes,
    )?;
    ensure_managed_path_components(
        Path::new(&move_row.destination_selected_root_path),
        Path::new(&move_row.destination_managed_root_path),
    )?;
    ensure_managed_root(
        Path::new(&move_row.destination_managed_root_path),
        &move_row.authority_key,
        &move_row.project_id,
    )?;

    let staging = Path::new(&move_row.destination_managed_root_path)
        .join("staging")
        .join(&move_row.move_id);
    remove_if_exists(&staging)?;
    ensure_private_directory(&staging)?;
    update_move_state(
        &state.inner.pool,
        move_id,
        DaemonProjectStorageMoveState::Materializing,
    )
    .await?;

    let source_generations = Path::new(&move_row.source_managed_root_path).join("generations");
    let staged_generations = staging.join("generations");
    let copied_generations = staged_generations.clone();
    tokio::task::spawn_blocking(move || copy_directory(&source_generations, &copied_generations))
        .await
        .map_err(|error| {
            storage_error(
                "storage_verification_failed",
                format!("Project generation copy task failed: {error}"),
            )
        })??;
    let source_search = Path::new(&move_row.source_managed_root_path).join("search/index.sqlite");
    let staged_search = staging.join("search/index.sqlite");
    let staged_search_parent = staged_search
        .parent()
        .expect("staged search index has a parent");
    ensure_private_directory(staged_search_parent)?;
    {
        let _storage_snapshot = state.inner.storage_access.write().await;
        if source_search.exists() {
            let source_pool = super::search::index::connect_project_index(&source_search).await?;
            sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .execute(&source_pool)
                .await?;
            source_pool.close().await;
            std::fs::copy(&source_search, &staged_search).map_err(|error| {
                storage_error(
                    "storage_verification_failed",
                    format!(
                        "Failed to copy search index from {} to {}: {error}",
                        source_search.display(),
                        staged_search.display()
                    ),
                )
            })?;
        }
    }
    let mut staged_effective_hash =
        super::search::materialize_project_index_at(state, &move_row.project_id, &staged_search)
            .await?;

    update_move_state(
        &state.inner.pool,
        move_id,
        DaemonProjectStorageMoveState::Verifying,
    )
    .await?;
    super::commit_sync::verify_current_project_generation_at(
        state,
        &move_row.project_id,
        &staged_generations,
    )
    .await
    .map_err(|error| {
        storage_error(
            "storage_verification_failed",
            format!(
                "Commit generation verification failed at {}: {error}",
                staged_generations.display()
            ),
        )
    })?;
    super::search::verify_project_index_at(
        &move_row.project_id,
        &staged_search,
        staged_effective_hash.as_deref(),
    )
    .await
    .map_err(|error| {
        storage_error(
            "storage_verification_failed",
            format!(
                "Search index verification failed at {}: {error}",
                staged_search.display()
            ),
        )
    })?;
    secure_managed_tree(&staging).map_err(|error| {
        storage_error(
            "storage_verification_failed",
            format!(
                "Managed storage permission verification failed at {}: {error}",
                staging.display()
            ),
        )
    })?;

    let _storage = state.inner.storage_access.write().await;
    // The background index worker may have published a newer head after the
    // optimistic snapshot above. Refresh and re-verify while holding the
    // storage write barrier so the destination cannot be promoted with an
    // obsolete (or newly missing) search database.
    if source_search.exists() {
        let source_pool = super::search::index::connect_project_index(&source_search).await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&source_pool)
            .await?;
        source_pool.close().await;
        std::fs::copy(&source_search, &staged_search).map_err(|error| {
            storage_error(
                "storage_verification_failed",
                format!(
                    "Failed to refresh search index snapshot from {} to {}: {error}",
                    source_search.display(),
                    staged_search.display()
                ),
            )
        })?;
    } else {
        remove_if_exists(&staged_search)?;
    }
    staged_effective_hash =
        super::search::materialize_project_index_at(state, &move_row.project_id, &staged_search)
            .await?;
    super::search::verify_project_index_at(
        &move_row.project_id,
        &staged_search,
        staged_effective_hash.as_deref(),
    )
    .await?;
    secure_managed_tree(&staging)?;
    update_move_state(
        &state.inner.pool,
        move_id,
        DaemonProjectStorageMoveState::Switching,
    )
    .await?;
    promote_staging(&staging, Path::new(&move_row.destination_managed_root_path))?;
    switch_location(state, &move_row).await?;
    super::search::publish_project_index_head(state, &move_row.project_id).await?;
    super::search::scheduler::enqueue_project(state, &move_row.project_id).await?;

    finish_cleanup(&state.inner.pool, move_id, &move_row).await?;
    Ok(())
}

async fn finish_cleanup(
    pool: &SqlitePool,
    move_id: &str,
    move_row: &StorageMoveRow,
) -> Result<(), DaemonError> {
    match cleanup_source(move_row) {
        Ok(()) => complete_move(pool, move_id).await?,
        Err(error) => {
            let (_, message) = storage_error_parts(&error);
            complete_move_with_cleanup_warning(pool, move_id, message).await?;
        }
    }
    Ok(())
}

async fn switch_location(
    state: &DaemonState,
    move_row: &StorageMoveRow,
) -> Result<(), DaemonError> {
    let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
    let current_revision: Option<i64> = sqlx::query_scalar(
        "SELECT location_revision FROM project_storage_locations
         WHERE authority_key = $1 AND project_id = $2",
    )
    .bind(&move_row.authority_key)
    .bind(&move_row.project_id)
    .fetch_optional(&mut *tx)
    .await?;
    let current_revision = current_revision.unwrap_or(DEFAULT_LOCATION_REVISION);
    ensure_expected_revision(move_row.source_location_revision, current_revision)?;
    let next_revision = current_revision + 1;
    let selected_root = (move_row.destination_mode == DaemonProjectStorageMode::Custom)
        .then_some(move_row.destination_selected_root_path.as_str());
    let bookmark = (move_row.destination_mode == DaemonProjectStorageMode::Custom)
        .then_some(move_row.destination_bookmark_data.as_deref())
        .flatten();
    sqlx::query(
        "INSERT INTO project_storage_locations (
            authority_key, project_id, mode, selected_root_path, bookmark_data, location_revision
         ) VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT(authority_key, project_id) DO UPDATE SET
            mode = excluded.mode,
            selected_root_path = excluded.selected_root_path,
            bookmark_data = excluded.bookmark_data,
            location_revision = excluded.location_revision,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(&move_row.authority_key)
    .bind(&move_row.project_id)
    .bind(move_row.destination_mode.as_str())
    .bind(selected_root)
    .bind(bookmark)
    .bind(next_revision)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE project_storage_moves
         SET state = 'cleaning', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE move_id = $1",
    )
    .bind(&move_row.move_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

fn cleanup_source(move_row: &StorageMoveRow) -> Result<(), DaemonError> {
    let source = Path::new(&move_row.source_managed_root_path);
    let destination = Path::new(&move_row.destination_managed_root_path);
    if paths_equivalent(source, destination) || !source.exists() {
        return Ok(());
    }
    verify_ownership_marker(source, &move_row.authority_key, &move_row.project_id)?;
    remove_if_exists(source)
}

async fn complete_move(pool: &SqlitePool, move_id: &str) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE project_storage_moves
         SET state = 'completed', error_code = NULL, error_message = NULL,
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE move_id = $1",
    )
    .bind(move_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn complete_move_with_cleanup_warning(
    pool: &SqlitePool,
    move_id: &str,
    message: String,
) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE project_storage_moves
         SET state = 'completed', error_code = 'storage_cleanup_failed', error_message = $2,
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE move_id = $1",
    )
    .bind(move_id)
    .bind(message)
    .execute(pool)
    .await?;
    Ok(())
}

async fn update_move_state(
    pool: &SqlitePool,
    move_id: &str,
    state: DaemonProjectStorageMoveState,
) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE project_storage_moves
         SET state = $2, error_code = NULL, error_message = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE move_id = $1",
    )
    .bind(move_id)
    .bind(state.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

fn validate_move_paths(
    move_row: &StorageMoveRow,
    require_source_marker: bool,
) -> Result<(SecurityScope, SecurityScope), DaemonError> {
    let source = resolve_selected_root(
        &move_row.source_selected_root_path,
        move_row.source_bookmark_data.as_deref(),
        move_row.source_mode == DaemonProjectStorageMode::Custom,
    )?;
    let destination = resolve_selected_root(
        &move_row.destination_selected_root_path,
        move_row.destination_bookmark_data.as_deref(),
        move_row.destination_mode == DaemonProjectStorageMode::Custom,
    )?;
    let expected_source = managed_root_for(
        move_row.source_mode,
        &source.path,
        &move_row.authority_key,
        &move_row.project_id,
    );
    let expected_destination = managed_root_for(
        move_row.destination_mode,
        &destination.path,
        &move_row.authority_key,
        &move_row.project_id,
    );
    if !paths_equivalent(
        &expected_source,
        Path::new(&move_row.source_managed_root_path),
    ) || !paths_equivalent(
        &expected_destination,
        Path::new(&move_row.destination_managed_root_path),
    ) {
        return Err(storage_error(
            "marker_mismatch",
            "Project storage move paths no longer match their configured roots",
        ));
    }
    validate_managed_path_components(&source.path, &expected_source)?;
    validate_managed_path_components(&destination.path, &expected_destination)?;
    if require_source_marker {
        verify_ownership_marker(
            Path::new(&move_row.source_managed_root_path),
            &move_row.authority_key,
            &move_row.project_id,
        )?;
    }
    Ok((source._security_scope, destination._security_scope))
}

fn promote_staging(staging: &Path, destination: &Path) -> Result<(), DaemonError> {
    for child in ["generations", "search"] {
        let staged = staging.join(child);
        let final_path = destination.join(child);
        remove_if_exists(&final_path)?;
        fs::rename(&staged, &final_path)?;
    }
    remove_if_exists(staging)?;
    secure_managed_tree(destination)?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), DaemonError> {
    remove_if_exists(destination)?;
    ensure_private_directory(destination)?;
    if !source.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            return Err(storage_error(
                "marker_mismatch",
                format!(
                    "Managed storage contains a symbolic link: {}",
                    entry.path().display()
                ),
            ));
        }
        if file_type.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target)?;
            set_mode(&target, 0o600)?;
        }
    }
    Ok(())
}

async fn resolve_setting(
    state: &DaemonState,
    authority_key: &str,
    project_id: &str,
    setting: Option<StorageSetting>,
) -> Result<ActiveProjectStorage, DaemonError> {
    let mut projection = projected_storage(state, authority_key, project_id, setting.as_ref())?;
    if let Some(setting) = setting
        && setting.mode == DaemonProjectStorageMode::Custom
    {
        let bookmark = setting.bookmark_data.as_deref().ok_or_else(|| {
            storage_error(
                "permission_required",
                "Custom Project storage is missing its macOS authorization bookmark",
            )
        })?;
        let resolved = resolve_selected_root(
            &projection.selected_root.display().to_string(),
            Some(bookmark),
            true,
        )?;
        if !paths_equivalent(&resolved.path, &projection.selected_root) {
            return Err(storage_error(
                "marker_mismatch",
                "The Project storage bookmark resolves to a different directory",
            ));
        }
        if let Some(refreshed_bookmark) = resolved.bookmark_to_persist {
            sqlx::query(
                "UPDATE project_storage_locations
                 SET selected_root_path = $4, bookmark_data = $3,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE authority_key = $1 AND project_id = $2",
            )
            .bind(authority_key)
            .bind(project_id)
            .bind(refreshed_bookmark)
            .bind(resolved.path.display().to_string())
            .execute(&state.inner.pool)
            .await?;
        }
        projection._security_scope = resolved._security_scope;
    }
    if projection.mode == DaemonProjectStorageMode::Default {
        migrate_legacy_default_root(
            &projection.selected_root,
            authority_key,
            project_id,
            &projection.managed_root,
        )?;
    }
    validate_managed_path_components(&projection.selected_root, &projection.managed_root)?;
    ensure_managed_path_components(&projection.selected_root, &projection.managed_root)?;
    ensure_managed_root(&projection.managed_root, authority_key, project_id)?;
    Ok(projection)
}

fn projected_storage(
    state: &DaemonState,
    authority_key: &str,
    project_id: &str,
    setting: Option<&StorageSetting>,
) -> Result<ActiveProjectStorage, DaemonError> {
    let default_selected_root = default_selected_root(state)?;
    let (mode, selected_root, location_revision) = match setting {
        Some(setting) if setting.mode == DaemonProjectStorageMode::Custom => {
            let selected_root = setting.selected_root_path.as_deref().ok_or_else(|| {
                storage_error(
                    "project_storage_corrupt",
                    "Custom Project storage is missing its selected root",
                )
            })?;
            (
                setting.mode,
                PathBuf::from(selected_root),
                setting.location_revision,
            )
        }
        Some(setting) => (
            DaemonProjectStorageMode::Default,
            default_selected_root.clone(),
            setting.location_revision,
        ),
        None => (
            DaemonProjectStorageMode::Default,
            default_selected_root,
            DEFAULT_LOCATION_REVISION,
        ),
    };
    let managed_root = managed_root_for(mode, &selected_root, authority_key, project_id);
    Ok(ActiveProjectStorage {
        authority_key: authority_key.to_owned(),
        project_id: project_id.to_owned(),
        mode,
        selected_root,
        managed_root,
        location_revision,
        _security_scope: empty_security_scope(),
    })
}

fn managed_root_for(
    mode: DaemonProjectStorageMode,
    selected_root: &Path,
    authority_key: &str,
    project_id: &str,
) -> PathBuf {
    match mode {
        DaemonProjectStorageMode::Default => {
            default_managed_root(selected_root, authority_key, project_id)
        }
        DaemonProjectStorageMode::Custom => {
            custom_managed_root(selected_root, authority_key, project_id)
        }
    }
}

fn default_selected_root(state: &DaemonState) -> Result<PathBuf, DaemonError> {
    Ok(fs::canonicalize(&state.inner.config.cache_dir)?)
}

fn default_managed_root(selected_root: &Path, authority_key: &str, project_id: &str) -> PathBuf {
    selected_root
        .join("projects")
        .join(authority_hash(authority_key))
        .join(project_id)
}

fn custom_managed_root(selected_root: &Path, authority_key: &str, project_id: &str) -> PathBuf {
    selected_root
        .join(STORAGE_DIRECTORY)
        .join(authority_hash(authority_key))
        .join(project_id)
}

fn authority_hash(authority_key: &str) -> String {
    hex::encode(Sha256::digest(authority_key.as_bytes()))
}

fn migrate_legacy_default_root(
    selected_root: &Path,
    authority_key: &str,
    project_id: &str,
    destination: &Path,
) -> Result<(), DaemonError> {
    let source = selected_root.join("projects").join(project_id);
    if !source.exists() || destination.exists() || paths_equivalent(&source, destination) {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(&source)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(storage_error(
            "marker_mismatch",
            format!(
                "Legacy Project storage path {} is not a safe directory",
                source.display()
            ),
        ));
    }
    let marker_path = source.join("ownership.json");
    if marker_path.exists() {
        verify_ownership_marker(&source, authority_key, project_id)?;
    } else {
        let marker = OwnershipMarker {
            layout_version: STORAGE_LAYOUT_VERSION,
            authority_key: authority_key.to_owned(),
            project_id: project_id.to_owned(),
        };
        write_private_file(&marker_path, &serde_json::to_vec_pretty(&marker)?)?;
    }
    secure_managed_tree(&source)?;
    let destination_parent = destination.parent().ok_or_else(|| {
        storage_error(
            "storage_verification_failed",
            "Project storage destination has no parent directory",
        )
    })?;
    ensure_managed_path_components(selected_root, destination_parent)?;
    match fs::rename(&source, destination) {
        Ok(()) => Ok(()),
        Err(_) if !source.exists() && destination.exists() => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn authority_key(state: &DaemonState) -> Result<String, DaemonError> {
    canonical_server_url(&state.project_config().server_url)
}

async fn load_setting(
    pool: &SqlitePool,
    authority_key: &str,
    project_id: &str,
) -> Result<Option<StorageSetting>, DaemonError> {
    let row = sqlx::query(
        "SELECT mode, selected_root_path,
                CAST(bookmark_data AS TEXT) AS bookmark_data, location_revision
         FROM project_storage_locations
         WHERE authority_key = $1 AND project_id = $2",
    )
    .bind(authority_key)
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(StorageSetting {
            mode: DaemonProjectStorageMode::parse(row.try_get("mode")?)?,
            selected_root_path: row.try_get("selected_root_path")?,
            bookmark_data: row.try_get("bookmark_data")?,
            location_revision: row.try_get("location_revision")?,
        })
    })
    .transpose()
}

async fn active_move_id(
    pool: &SqlitePool,
    authority_key: &str,
    project_id: &str,
) -> Result<Option<String>, DaemonError> {
    Ok(sqlx::query_scalar(
        "SELECT move_id FROM project_storage_moves
         WHERE authority_key = $1 AND project_id = $2
           AND state NOT IN ('completed', 'failed')
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(authority_key)
    .bind(project_id)
    .fetch_optional(pool)
    .await?)
}

async fn latest_completed_move_issue(
    pool: &SqlitePool,
    authority_key: &str,
    project_id: &str,
) -> Result<Option<(String, String)>, DaemonError> {
    let row = sqlx::query(
        "SELECT error_code, error_message
         FROM project_storage_moves
         WHERE authority_key = $1 AND project_id = $2 AND state = 'completed'
         ORDER BY completed_at DESC, created_at DESC
         LIMIT 1",
    )
    .bind(authority_key)
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let code: Option<String> = row.try_get("error_code")?;
        let message: Option<String> = row.try_get("error_message")?;
        Ok(code.zip(message))
    })
    .transpose()
    .map(Option::flatten)
}

async fn active_generation_path(
    state: &DaemonState,
    storage: &ActiveProjectStorage,
) -> Result<Option<String>, DaemonError> {
    let commit_id: Option<String> = sqlx::query_scalar(
        "SELECT commit_id FROM cached_refs
         WHERE scope = 'project' AND project_id = $1",
    )
    .bind(&storage.project_id)
    .fetch_optional(&state.inner.pool)
    .await?
    .flatten();
    let generation = commit_id.as_deref().unwrap_or("ref-none");
    let path = storage.generation_path(generation);
    Ok(path.exists().then(|| path.display().to_string()))
}

async fn load_move_view(
    pool: &SqlitePool,
    move_id: &str,
) -> Result<DaemonProjectStorageMove, DaemonError> {
    let row = sqlx::query(
        "SELECT move_id, project_id, source_mode, destination_mode,
                source_managed_root_path, destination_managed_root_path,
                source_location_revision, state, error_code, error_message,
                created_at, updated_at, completed_at
         FROM project_storage_moves WHERE move_id = $1",
    )
    .bind(move_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DaemonError::NotFound(format!("Project storage move {move_id}")))?;
    Ok(DaemonProjectStorageMove {
        move_id: row.try_get("move_id")?,
        project_id: row.try_get("project_id")?,
        source_mode: DaemonProjectStorageMode::parse(row.try_get("source_mode")?)?,
        destination_mode: DaemonProjectStorageMode::parse(row.try_get("destination_mode")?)?,
        source_managed_root_path: row.try_get("source_managed_root_path")?,
        destination_managed_root_path: row.try_get("destination_managed_root_path")?,
        source_location_revision: row.try_get("source_location_revision")?,
        state: DaemonProjectStorageMoveState::parse(row.try_get("state")?)?,
        error_code: row.try_get("error_code")?,
        error_message: row.try_get("error_message")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

async fn load_move_row(pool: &SqlitePool, move_id: &str) -> Result<StorageMoveRow, DaemonError> {
    let row = sqlx::query(
        "SELECT move_id, authority_key, project_id,
                source_mode, source_selected_root_path,
                CAST(source_bookmark_data AS TEXT) AS source_bookmark_data,
                source_managed_root_path, source_location_revision,
                destination_mode, destination_selected_root_path,
                CAST(destination_bookmark_data AS TEXT) AS destination_bookmark_data,
                destination_managed_root_path, state
         FROM project_storage_moves WHERE move_id = $1",
    )
    .bind(move_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DaemonError::NotFound(format!("Project storage move {move_id}")))?;
    Ok(StorageMoveRow {
        move_id: row.try_get("move_id")?,
        authority_key: row.try_get("authority_key")?,
        project_id: row.try_get("project_id")?,
        source_mode: DaemonProjectStorageMode::parse(row.try_get("source_mode")?)?,
        source_selected_root_path: row.try_get("source_selected_root_path")?,
        source_bookmark_data: row.try_get("source_bookmark_data")?,
        source_managed_root_path: row.try_get("source_managed_root_path")?,
        source_location_revision: row.try_get("source_location_revision")?,
        destination_mode: DaemonProjectStorageMode::parse(row.try_get("destination_mode")?)?,
        destination_selected_root_path: row.try_get("destination_selected_root_path")?,
        destination_bookmark_data: row.try_get("destination_bookmark_data")?,
        destination_managed_root_path: row.try_get("destination_managed_root_path")?,
        state: DaemonProjectStorageMoveState::parse(row.try_get("state")?)?,
    })
}

fn resolve_selected_root(
    selected_root_path: &str,
    bookmark_data: Option<&str>,
    bookmark_required: bool,
) -> Result<ResolvedSelectedRoot, DaemonError> {
    let supplied = requested_selected_root(selected_root_path)?;
    let resolved = match bookmark_data {
        Some(bookmark) if !bookmark.trim().is_empty() => match resolve_bookmark(bookmark) {
            Ok(resolved) => resolved,
            Err(BookmarkFailure::Fatal(error)) => return Err(error),
            Err(BookmarkFailure::Stale(error)) => repair_stale_bookmark(supplied.clone(), error)?,
        },
        _ if bookmark_required => {
            return Err(storage_error(
                "permission_required",
                "A macOS directory authorization bookmark is required",
            ));
        }
        _ => ResolvedSelectedRoot {
            path: supplied.clone(),
            bookmark_to_persist: None,
            _security_scope: empty_security_scope(),
        },
    };
    validate_resolved_selected_root(supplied, resolved)
}

fn resolve_handoff_selected_root(
    selected_root_path: &str,
    handoff_bookmark_data: &str,
) -> Result<ResolvedSelectedRoot, DaemonError> {
    let supplied = requested_selected_root(selected_root_path)?;
    if handoff_bookmark_data.trim().is_empty() {
        return Err(storage_error(
            "permission_required",
            "A macOS directory handoff bookmark is required",
        ));
    }
    let resolved = resolve_handoff_bookmark(handoff_bookmark_data)?;
    validate_resolved_selected_root(supplied, resolved)
}

fn requested_selected_root(selected_root_path: &str) -> Result<PathBuf, DaemonError> {
    let selected_root_path = selected_root_path.trim();
    if selected_root_path.is_empty() {
        return Err(DaemonError::InvalidRequest(
            "selected_root_path must not be empty".to_owned(),
        ));
    }
    Ok(PathBuf::from(selected_root_path))
}

fn validate_resolved_selected_root(
    supplied: PathBuf,
    resolved: ResolvedSelectedRoot,
) -> Result<ResolvedSelectedRoot, DaemonError> {
    let candidate = resolved.path;
    if !candidate.exists() {
        return Err(storage_error(
            "volume_missing",
            format!(
                "Project storage directory {} is unavailable",
                candidate.display()
            ),
        ));
    }
    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        storage_error(
            if error.kind() == std::io::ErrorKind::NotFound {
                "volume_missing"
            } else {
                "permission_required"
            },
            format!(
                "Project storage directory {} cannot be resolved: {error}",
                candidate.display()
            ),
        )
    })?;
    if !canonical.is_dir() {
        return Err(DaemonError::InvalidRequest(format!(
            "Project storage path {} is not a directory",
            canonical.display()
        )));
    }
    let supplied_canonical = fs::canonicalize(&supplied).map_err(|error| {
        storage_error(
            "volume_missing",
            format!(
                "Selected Project storage directory {} cannot be resolved: {error}",
                supplied.display()
            ),
        )
    })?;
    if !paths_equivalent(&canonical, &supplied_canonical) {
        return Err(storage_error(
            "marker_mismatch",
            "The directory authorization bookmark does not match the selected path",
        ));
    }
    ensure_local_volume(&canonical)?;
    verify_directory_writable(&canonical)?;
    Ok(ResolvedSelectedRoot {
        path: canonical,
        bookmark_to_persist: resolved.bookmark_to_persist,
        _security_scope: resolved._security_scope,
    })
}

fn validate_custom_selected_root(
    state: &DaemonState,
    selected_root: &Path,
    current_managed_root: &Path,
) -> Result<(), DaemonError> {
    if selected_root.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".app")
    }) {
        return Err(DaemonError::InvalidRequest(
            "Project storage cannot be placed inside an application bundle".to_owned(),
        ));
    }
    let runtime_root = fs::canonicalize(&state.inner.config.root_dir)
        .unwrap_or_else(|_| state.inner.config.root_dir.clone());
    if selected_root.starts_with(current_managed_root)
        || current_managed_root.starts_with(selected_root.join(STORAGE_DIRECTORY))
        || selected_root.starts_with(runtime_root)
    {
        return Err(DaemonError::InvalidRequest(
            "Project storage cannot be nested inside Clumsies runtime data or an active managed cache"
                .to_owned(),
        ));
    }
    Ok(())
}

fn validate_managed_path_components(
    selected_root: &Path,
    managed_root: &Path,
) -> Result<(), DaemonError> {
    let relative = managed_root.strip_prefix(selected_root).map_err(|_| {
        storage_error(
            "marker_mismatch",
            "Project managed storage is outside its selected root",
        )
    })?;
    let mut current = selected_root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(storage_error(
                    "marker_mismatch",
                    format!(
                        "Project storage path contains a symbolic link: {}",
                        current.display()
                    ),
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(storage_error(
                    "marker_mismatch",
                    format!(
                        "Project storage path component {} is not a directory",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn ensure_managed_path_components(
    selected_root: &Path,
    managed_root: &Path,
) -> Result<(), DaemonError> {
    let relative = managed_root.strip_prefix(selected_root).map_err(|_| {
        storage_error(
            "marker_mismatch",
            "Project managed storage is outside its selected root",
        )
    })?;
    let mut current = selected_root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(storage_error(
                "marker_mismatch",
                format!(
                    "Project storage path component {} is not a safe directory",
                    current.display()
                ),
            ));
        }
        set_mode(&current, 0o700)?;
    }
    Ok(())
}

fn verify_directory_writable(path: &Path) -> Result<(), DaemonError> {
    let probe = path.join(format!(".clumsies-write-probe-{}", Uuid::new_v4().simple()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            fs::remove_file(probe)?;
            Ok(())
        }
        Err(error) => Err(storage_error(
            io_storage_code(&error),
            format!(
                "Project storage directory {} is not writable: {error}",
                path.display()
            ),
        )),
    }
}

fn verify_ownership_marker(
    root: &Path,
    authority_key: &str,
    project_id: &str,
) -> Result<(), DaemonError> {
    let marker_path = root.join("ownership.json");
    let contents = fs::read(&marker_path).map_err(|error| {
        storage_error(
            "marker_mismatch",
            format!(
                "Project storage marker {} cannot be read: {error}",
                marker_path.display()
            ),
        )
    })?;
    let marker: OwnershipMarker = serde_json::from_slice(&contents).map_err(|error| {
        storage_error(
            "marker_mismatch",
            format!(
                "Project storage marker {} is invalid: {error}",
                marker_path.display()
            ),
        )
    })?;
    if marker.layout_version != STORAGE_LAYOUT_VERSION
        || marker.authority_key != authority_key
        || marker.project_id != project_id
    {
        return Err(storage_error(
            "marker_mismatch",
            format!(
                "Project storage marker {} belongs to a different authority or Project",
                marker_path.display()
            ),
        ));
    }
    Ok(())
}

fn is_legacy_default_root(root: &Path) -> bool {
    root.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "projects")
}

fn ensure_expected_revision(expected: i64, actual: i64) -> Result<(), DaemonError> {
    if expected == actual {
        Ok(())
    } else {
        Err(storage_error(
            "storage_move_conflict",
            format!("Project storage location revision changed from {expected} to {actual}"),
        ))
    }
}

fn managed_size(root: &Path) -> Result<u64, DaemonError> {
    if !root.exists() {
        return Ok(0);
    }
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        return Err(storage_error(
            "marker_mismatch",
            format!(
                "Managed storage contains a symbolic link: {}",
                root.display()
            ),
        ));
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut total = 0u64;
    for entry in fs::read_dir(root)? {
        total = total.saturating_add(managed_size(&entry?.path())?);
    }
    Ok(total)
}

fn remove_if_exists(path: &Path) -> Result<(), DaemonError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(storage_error(
            "marker_mismatch",
            format!("Refusing to remove symbolic link {}", path.display()),
        )),
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path)?;
            Ok(())
        }
        Ok(_) => {
            fs::remove_file(path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    left == right
        || (left.exists()
            && right.exists()
            && fs::canonicalize(left).ok() == fs::canonicalize(right).ok())
}

fn ensure_available_space(path: &Path, required_bytes: u64) -> Result<(), DaemonError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            DaemonError::InvalidRequest("Project storage path contains a NUL byte".to_owned())
        })?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stats = unsafe { stats.assume_init() };
        let available = (stats.f_bavail as u64).saturating_mul(stats.f_frsize);
        if available < required_bytes {
            return Err(storage_error(
                "insufficient_space",
                format!(
                    "Project storage needs {required_bytes} bytes but only {available} bytes are available"
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn ensure_local_volume(path: &Path) -> Result<(), DaemonError> {
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        DaemonError::InvalidRequest("Project storage path contains a NUL byte".to_owned())
    })?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    let result = unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stats = unsafe { stats.assume_init() };
    if stats.f_flags & libc::MNT_LOCAL as u32 == 0 {
        return Err(DaemonError::InvalidRequest(
            "Project storage must be located on a local filesystem".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn ensure_local_volume(_path: &Path) -> Result<(), DaemonError> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn resolve_bookmark(bookmark: &str) -> Result<ResolvedSelectedRoot, BookmarkFailure> {
    use objc2::runtime::Bool;
    use objc2_foundation::{
        NSData, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
    };

    let bytes = STANDARD.decode(bookmark.trim()).map_err(|error| {
        BookmarkFailure::Fatal(storage_error(
            "project_storage_corrupt",
            format!("Project storage bookmark is not valid Base64: {error}"),
        ))
    })?;
    let data = NSData::with_bytes(&bytes);
    let mut stale = Bool::NO;
    let options = NSURLBookmarkResolutionOptions::WithSecurityScope
        | NSURLBookmarkResolutionOptions::WithoutUI
        | NSURLBookmarkResolutionOptions::WithoutMounting
        | NSURLBookmarkResolutionOptions::WithoutImplicitStartAccessing;
    let url = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            &data, options, None, &mut stale,
        )
    }
    .map_err(|error| classify_stale_bookmark(&data, error))?;
    if !unsafe { url.startAccessingSecurityScopedResource() } {
        return Err(BookmarkFailure::Fatal(storage_error(
            "permission_required",
            "macOS denied access to the selected Project storage directory",
        )));
    }
    let access = Arc::new(SecurityScopedAccess { url });
    let path = access.url.path().ok_or_else(|| {
        storage_error(
            "permission_required",
            "Project storage bookmark does not resolve to a filesystem path",
        )
    })?;
    let refreshed_bookmark = if stale.as_bool() {
        let data = access
            .url
            .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                NSURLBookmarkCreationOptions::WithSecurityScope,
                None,
                None,
            )
            .map_err(|error| {
                storage_error(
                    "permission_required",
                    format!("Stale Project storage bookmark cannot be refreshed: {error}"),
                )
            })?;
        Some(STANDARD.encode(data.to_vec()))
    } else {
        None
    };
    Ok(ResolvedSelectedRoot {
        path: PathBuf::from(path.to_string()),
        bookmark_to_persist: refreshed_bookmark,
        _security_scope: Some(access),
    })
}

#[cfg(target_os = "macos")]
fn resolve_handoff_bookmark(bookmark: &str) -> Result<ResolvedSelectedRoot, DaemonError> {
    use objc2::runtime::Bool;
    use objc2_foundation::{
        NSData, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
    };

    let bytes = STANDARD.decode(bookmark).map_err(|error| {
        storage_error(
            "permission_required",
            format!("Project storage handoff bookmark is not valid Base64: {error}"),
        )
    })?;
    let data = NSData::with_bytes(&bytes);
    let mut stale = Bool::NO;
    let options =
        NSURLBookmarkResolutionOptions::WithoutUI | NSURLBookmarkResolutionOptions::WithoutMounting;
    let url = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            &data, options, None, &mut stale,
        )
    }
    .map_err(|error| {
        storage_error(
            "permission_required",
            format!("Project storage handoff bookmark cannot be resolved: {error}"),
        )
    })?;
    let daemon_bookmark = url
        .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions::WithSecurityScope,
            None,
            None,
        )
        .map_err(|error| {
            storage_error(
                "permission_required",
                format!("Daemon Project storage authorization cannot be created: {error}"),
            )
        })?;
    let daemon_bookmark = STANDARD.encode(daemon_bookmark.to_vec());
    let mut resolved = match resolve_bookmark(&daemon_bookmark) {
        Ok(resolved) => resolved,
        Err(BookmarkFailure::Fatal(error) | BookmarkFailure::Stale(error)) => return Err(error),
    };
    resolved.bookmark_to_persist = Some(daemon_bookmark);
    Ok(resolved)
}

/// Classify a failed security-scoped bookmark resolution. A bookmark whose
/// referenced file identity no longer exists is stale and may be repaired from
/// the persisted selected path; undecodable or structurally invalid data is
/// corrupt and requires re-selecting the directory.
#[cfg(target_os = "macos")]
fn classify_stale_bookmark(
    data: &objc2_foundation::NSData,
    scoped_error: objc2::rc::Retained<objc2_foundation::NSError>,
) -> BookmarkFailure {
    use objc2::runtime::Bool;
    use objc2_foundation::{NSCocoaErrorDomain, NSURL, NSURLBookmarkResolutionOptions};

    let mut stale = Bool::NO;
    let plain_options = NSURLBookmarkResolutionOptions::WithoutUI
        | NSURLBookmarkResolutionOptions::WithoutMounting
        | NSURLBookmarkResolutionOptions::WithoutImplicitStartAccessing;
    let plain = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            data,
            plain_options,
            None,
            &mut stale,
        )
    };
    let message = format!("Project storage bookmark cannot be resolved: {scoped_error}");
    let plain_error_is_missing_file = plain.as_ref().err().is_some_and(|error| {
        let cocoa_domain = unsafe { NSCocoaErrorDomain };
        error.domain().to_string() == cocoa_domain.to_string() && error.code() == 4
    });
    match plain {
        // The bookmark still resolves by path; only its security scope is stale.
        Ok(_) => BookmarkFailure::Stale(storage_error("permission_required", message)),
        // The referenced file identity no longer exists anywhere.
        Err(_) if plain_error_is_missing_file => {
            BookmarkFailure::Stale(storage_error("permission_required", message))
        }
        Err(_) => BookmarkFailure::Fatal(storage_error("project_storage_corrupt", message)),
    }
}

/// Re-authorize a stale bookmark from the persisted selected path. The path is
/// the trust anchor: it is persisted together with the bookmark during
/// `replace_project_storage`, and the caller validates the result is a local,
/// writable directory before it is used.
#[cfg(target_os = "macos")]
fn repair_stale_bookmark(
    supplied: PathBuf,
    stale_error: DaemonError,
) -> Result<ResolvedSelectedRoot, DaemonError> {
    let canonical = fs::canonicalize(&supplied).map_err(|error| {
        storage_error(
            if error.kind() == std::io::ErrorKind::NotFound {
                "volume_missing"
            } else {
                "permission_required"
            },
            format!(
                "Project storage bookmark is stale ({stale_error}) and its selected directory {} cannot be resolved: {error}",
                supplied.display()
            ),
        )
    })?;
    if !canonical.is_dir() {
        return Err(DaemonError::InvalidRequest(format!(
            "Project storage path {} is not a directory",
            canonical.display()
        )));
    }
    tracing::warn!(
        "Project storage bookmark is stale ({stale_error}); re-authorizing selected directory {}",
        canonical.display()
    );
    create_scoped_bookmark(&canonical)
}

/// Create a fresh security-scoped bookmark for a canonical directory path.
#[cfg(target_os = "macos")]
fn create_scoped_bookmark(canonical: &Path) -> Result<ResolvedSelectedRoot, DaemonError> {
    use objc2_foundation::{NSString, NSURL, NSURLBookmarkCreationOptions};

    let path = NSString::from_str(&canonical.display().to_string());
    let url = NSURL::fileURLWithPath_isDirectory(&path, true);
    let bookmark = url
        .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions::WithSecurityScope,
            None,
            None,
        )
        .map_err(|error| {
            storage_error(
                "permission_required",
                format!(
                    "Project storage authorization cannot be created for {}: {error}",
                    canonical.display()
                ),
            )
        })?;
    let access = Arc::new(SecurityScopedAccess { url });
    if !unsafe { access.url.startAccessingSecurityScopedResource() } {
        return Err(storage_error(
            "permission_required",
            "macOS denied access to the re-authorized Project storage directory",
        ));
    }
    Ok(ResolvedSelectedRoot {
        path: canonical.to_path_buf(),
        bookmark_to_persist: Some(STANDARD.encode(bookmark.to_vec())),
        _security_scope: Some(access),
    })
}

#[cfg(not(target_os = "macos"))]
fn resolve_bookmark(bookmark: &str) -> Result<ResolvedSelectedRoot, BookmarkFailure> {
    STANDARD.decode(bookmark).map_err(|error| {
        BookmarkFailure::Fatal(storage_error(
            "project_storage_corrupt",
            format!("Project storage bookmark is not valid Base64: {error}"),
        ))
    })?;
    Err(BookmarkFailure::Fatal(storage_error(
        "permission_required",
        "Directory authorization bookmarks are only supported by the macOS daemon",
    )))
}

#[cfg(not(target_os = "macos"))]
fn repair_stale_bookmark(
    _supplied: PathBuf,
    _stale_error: DaemonError,
) -> Result<ResolvedSelectedRoot, DaemonError> {
    Err(storage_error(
        "permission_required",
        "Directory authorization bookmarks are only supported by the macOS daemon",
    ))
}

#[cfg(not(target_os = "macos"))]
fn resolve_handoff_bookmark(bookmark: &str) -> Result<ResolvedSelectedRoot, DaemonError> {
    STANDARD.decode(bookmark).map_err(|error| {
        storage_error(
            "permission_required",
            format!("Project storage handoff bookmark is not valid Base64: {error}"),
        )
    })?;
    Err(storage_error(
        "permission_required",
        "Directory authorization bookmarks are only supported by the macOS daemon",
    ))
}

#[cfg(target_os = "macos")]
fn empty_security_scope() -> SecurityScope {
    None
}

#[cfg(not(target_os = "macos"))]
fn empty_security_scope() -> SecurityScope {}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), DaemonError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), DaemonError> {
    Ok(())
}

fn io_storage_code(error: &std::io::Error) -> &'static str {
    match error.raw_os_error() {
        Some(libc::ENOSPC) => "insufficient_space",
        Some(code) if code == libc::EACCES || code == libc::EPERM => "permission_required",
        Some(code) if code == libc::ENOENT || code == libc::ENODEV => "volume_missing",
        _ => "storage_verification_failed",
    }
}

fn storage_error(code: &'static str, message: impl Into<String>) -> DaemonError {
    DaemonError::State {
        code,
        message: message.into(),
    }
}

fn storage_error_parts(error: &DaemonError) -> (String, String) {
    match error {
        DaemonError::State { code, message } => ((*code).to_owned(), message.clone()),
        DaemonError::Io(error) => (io_storage_code(error).to_owned(), error.to_string()),
        _ => ("storage_verification_failed".to_owned(), error.to_string()),
    }
}
