mod activation;
mod chunker;
pub(crate) mod index;
pub(crate) mod models;
mod overlay;
mod query;
pub(crate) mod scheduler;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pulldown_cmark::{Event, Options, Parser, Tag};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

use self::models::{FastEmbedSearchModels, SearchModelRuntimeStatus, SearchModels};
use super::retrieval_history::{
    RetrievalCandidateInput, RetrievalCorpusResourceInput, RetrievalDeltaAction,
    RetrievalExclusionReason, RetrievalRunCompletion,
};
use super::{
    DaemonDraftContent, DaemonDraftOperation, DaemonError, DaemonMemoryCacheRequest,
    DaemonMemoryCacheState, DaemonMemoryCacheStatus, DaemonState, DaemonUpdateDraftOperation,
};

pub(super) const SEARCH_SCHEMA_VERSION: i64 = 3;
pub(super) const PARSER_VERSION: &str = "markdown-units.v1";
pub(super) const CHUNKER_VERSION: &str = "markdown-chunker.v1";
pub(super) const RANKING_CONFIG_VERSION: &str = "agent_activation.v2";
// BGE emits unbounded logits; the floor is applied after sigmoid normalization.
pub(super) const MIN_RERANK_RELEVANCE: f32 = 0.01;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Memory,
}

impl MemoryKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    Org,
    Project,
}

impl SourceScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Project => "project",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceLocator {
    MarkdownSpan {
        start_byte: usize,
        end_byte: usize,
        heading_path: Vec<String>,
    },
}

impl SourceLocator {
    #[cfg(test)]
    pub(crate) fn start_byte(&self) -> usize {
        match self {
            Self::MarkdownSpan { start_byte, .. } => *start_byte,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceResource {
    pub(crate) resource_id: String,
    pub(crate) project_id: String,
    pub(crate) scope: SourceScope,
    pub(crate) kind: MemoryKind,
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) content: String,
    pub(crate) content_hash: String,
    pub(crate) source_commit_id: Option<String>,
    pub(crate) draft_id: Option<String>,
    pub(crate) draft_revision: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RetrievalUnit {
    pub(crate) unit_key: String,
    pub(crate) resource_id: String,
    pub(crate) ordinal: usize,
    pub(crate) heading_path: Vec<String>,
    pub(crate) locator: SourceLocator,
    pub(crate) text: String,
    pub(crate) text_hash: String,
    pub(crate) token_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ActivateMemoryRequest {
    pub project_id: String,
    pub query: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ActivateMemoryResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub index_revision: String,
    pub profile: String,
    pub next_state: String,
    pub fragments: Vec<ActivationFragment>,
    pub removed: Vec<ActivationRemoval>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivationAction {
    Add,
    Replace,
    Reuse,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ActivationFragment {
    pub action: ActivationAction,
    pub unit_key: String,
    pub content_hash: String,
    pub resource_id: String,
    pub scope: SourceScope,
    pub kind: MemoryKind,
    pub path: String,
    pub heading_path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ActivationRemoval {
    pub unit_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LoadMemoryRequest {
    pub project_id: String,
    pub ids: Vec<String>,
    #[serde(default)]
    pub known_hashes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LoadMemoryResponse {
    pub resources: Vec<LoadedMemoryResource>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LoadedMemoryResource {
    pub resource_id: String,
    pub scope: SourceScope,
    pub kind: MemoryKind,
    pub path: String,
    pub title: String,
    pub description: String,
    pub content_hash: String,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SearchIndexProjectRequest {
    pub project_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchModelStatus {
    Missing,
    Preparing,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SearchIndexStatus {
    pub project_id: String,
    pub effective_hash: String,
    pub active_revision: Option<String>,
    pub active_effective_hash: Option<String>,
    pub ready: bool,
    pub model_status: SearchModelStatus,
    pub model_downloaded_bytes: Option<u64>,
    pub model_total_bytes: Option<u64>,
    pub build_state: Option<String>,
    pub desired_sequence: Option<i64>,
    pub completed_sequence: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct EffectiveMemory {
    pub(crate) project_id: String,
    pub(crate) effective_hash: String,
    pub(crate) resources: Arc<[SourceResource]>,
}

#[derive(Clone, Debug)]
pub(super) struct EffectiveResource {
    pub(super) source: SourceResource,
}

#[derive(Debug)]
pub(crate) struct SearchFailure {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl SearchFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn model(message: impl Into<String>) -> Self {
        Self::new("search_model_unavailable", message)
    }

    pub(super) fn model_preparing(message: impl Into<String>) -> Self {
        Self::new("search_model_preparing", message)
    }

    pub(crate) fn vector(message: impl Into<String>) -> Self {
        Self::new("search_vector_corrupt", message)
    }

    pub(super) fn invalid_state(message: impl Into<String>) -> Self {
        Self::new("invalid_activation_state", message)
    }

    pub(super) fn not_ready(message: impl Into<String>) -> Self {
        Self::new("search_index_not_ready", message)
    }

    pub(super) fn index_preparing(message: impl Into<String>) -> Self {
        Self::new("search_index_preparing", message)
    }

    pub(super) fn generation_changed(message: impl Into<String>) -> Self {
        Self::new("search_generation_changed", message)
    }

    fn resource_not_found(message: impl Into<String>) -> Self {
        Self::new("memory_resource_not_found", message)
    }

    pub(super) fn failed(message: impl Into<String>) -> Self {
        Self::new("search_index_failed", message)
    }
}

impl From<SearchFailure> for DaemonError {
    fn from(error: SearchFailure) -> Self {
        Self::Search {
            code: error.code.to_owned(),
            message: error.message,
        }
    }
}

pub(crate) fn production_models(cache_dir: PathBuf) -> Arc<dyn SearchModels> {
    Arc::new(FastEmbedSearchModels::new(cache_dir.join("models")))
}

pub(crate) async fn migrate(pool: &SqlitePool) -> Result<(), DaemonError> {
    let existing: Option<String> =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'search_schema_version'")
            .fetch_optional(pool)
            .await?;
    let version = existing.and_then(|value| value.parse::<i64>().ok());
    let removed_legacy_index = version.is_some() && version != Some(SEARCH_SCHEMA_VERSION);
    if version != Some(SEARCH_SCHEMA_VERSION) {
        let mut tx = pool.begin().await?;
        sqlx::query("DROP TABLE IF EXISTS search_units_fts")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DROP TABLE IF EXISTS search_heads")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DROP TABLE IF EXISTS search_units")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DROP TABLE IF EXISTS search_resources")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DROP TABLE IF EXISTS search_revisions")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }

    if removed_legacy_index {
        sqlx::query("VACUUM").execute(pool).await?;
    }

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS search_heads (
            project_id TEXT PRIMARY KEY,
            revision_id TEXT NOT NULL,
            effective_hash TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('ready', 'failed')),
            last_error TEXT,
            location_revision BIGINT NOT NULL CHECK (location_revision > 0),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO daemon_meta (key, value)
         VALUES ('search_schema_version', $1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(SEARCH_SCHEMA_VERSION.to_string())
    .execute(pool)
    .await?;
    scheduler::migrate(pool).await?;
    Ok(())
}

pub(super) async fn active_project_index(
    state: &DaemonState,
    project_id: &str,
) -> Result<(SqlitePool, super::project_storage::ActiveProjectStorage), DaemonError> {
    let storage = super::project_storage::resolve_active(state, project_id).await?;
    let pool = index::connect_project_index(&storage.search_index_path()).await?;
    Ok((pool, storage))
}

async fn mirror_project_search_head_if_current(
    state: &DaemonState,
    project_id: &str,
    location_revision: i64,
) -> Result<(), DaemonError> {
    // Serialize with scheduler publication, then reopen and reread the
    // project head while holding the central writer barrier. This prevents a
    // status read that observed R1 from overwriting a concurrently published
    // R2 central head after that scheduler commit.
    let mut barrier = state.inner.pool.begin().await?;
    sqlx::query("UPDATE search_index_jobs SET updated_at = updated_at WHERE project_id = $1")
        .bind(project_id)
        .execute(&mut *barrier)
        .await?;
    let (pool, storage) = active_project_index(state, project_id).await?;
    if storage.location_revision != location_revision {
        pool.close().await;
        barrier.rollback().await?;
        return Ok(());
    }
    let row = sqlx::query(
        "SELECT r.revision_id, r.effective_hash, r.status, r.last_error
         FROM search_heads h
         JOIN search_revisions r ON r.revision_id = h.revision_id
         WHERE h.project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(&pool)
    .await?;
    match row {
        Some(row) => {
            sqlx::query(
                "INSERT INTO search_heads (
                    project_id, revision_id, effective_hash, status, last_error, location_revision
                 ) VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT(project_id) DO UPDATE SET
                    revision_id = excluded.revision_id,
                    effective_hash = excluded.effective_hash,
                    status = excluded.status,
                    last_error = excluded.last_error,
                    location_revision = excluded.location_revision,
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            )
            .bind(project_id)
            .bind(row.try_get::<String, _>("revision_id")?)
            .bind(row.try_get::<String, _>("effective_hash")?)
            .bind(row.try_get::<String, _>("status")?)
            .bind(row.try_get::<Option<String>, _>("last_error")?)
            .bind(location_revision)
            .execute(&mut *barrier)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM search_heads WHERE project_id = $1")
                .bind(project_id)
                .execute(&mut *barrier)
                .await?;
        }
    }
    pool.close().await;
    barrier.commit().await?;
    Ok(())
}

pub(crate) async fn materialize_project_index_at(
    _state: &DaemonState,
    project_id: &str,
    path: &Path,
) -> Result<Option<String>, DaemonError> {
    let pool = index::connect_project_index(path).await?;
    let effective_hash: Option<String> = sqlx::query_scalar(
        "SELECT r.effective_hash
         FROM search_heads h
         JOIN search_revisions r ON r.revision_id = h.revision_id
         WHERE h.project_id = $1 AND r.status = 'ready'",
    )
    .bind(project_id)
    .fetch_optional(&pool)
    .await?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&pool)
        .await?;
    pool.close().await;
    super::project_storage::secure_managed_tree(path.parent().unwrap_or(path))?;
    Ok(effective_hash)
}

pub(crate) async fn verify_project_index_at(
    project_id: &str,
    path: &Path,
    expected_effective_hash: Option<&str>,
) -> Result<(), DaemonError> {
    if !path.exists() {
        return Err(DaemonError::State {
            code: "storage_verification_failed",
            message: format!("Project search index {} is missing", path.display()),
        });
    }
    let pool = index::connect_project_index(path).await?;
    let ready: i64 = match expected_effective_hash {
        Some(effective_hash) => {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM search_heads h
                 JOIN search_revisions r ON r.revision_id = h.revision_id
                 WHERE h.project_id = $1 AND r.effective_hash = $2 AND r.status = 'ready'",
            )
            .bind(project_id)
            .bind(effective_hash)
            .fetch_one(&pool)
            .await?
        }
        None => {
            sqlx::query_scalar("SELECT COUNT(*) FROM search_heads WHERE project_id = $1")
                .bind(project_id)
                .fetch_one(&pool)
                .await?
        }
    };
    pool.close().await;
    let verified = if expected_effective_hash.is_some() {
        ready == 1
    } else {
        ready == 0
    };
    if !verified {
        return Err(DaemonError::State {
            code: "storage_verification_failed",
            message: "Project search index does not match its migration snapshot".to_owned(),
        });
    }
    Ok(())
}

pub(crate) async fn publish_project_index_head(
    state: &DaemonState,
    project_id: &str,
) -> Result<(), DaemonError> {
    let storage = super::project_storage::resolve_active(state, project_id).await?;
    mirror_project_search_head_if_current(state, project_id, storage.location_revision).await
}

pub(crate) async fn activate_memory(
    state: &DaemonState,
    request: ActivateMemoryRequest,
) -> Result<ActivateMemoryResponse, DaemonError> {
    let query = request.query.trim();
    if request.project_id.trim().is_empty() || query.is_empty() {
        return Err(DaemonError::InvalidRequest(
            "project_id and a non-empty query are required".to_owned(),
        ));
    }
    let project_id = request.project_id.trim().to_owned();
    let query = query.to_owned();
    let total_started = Instant::now();
    let fingerprint =
        super::retrieval_history::activation_state_fingerprint(request.state.as_deref());
    let run_id =
        match super::retrieval_history::start_run(state, &project_id, &query, &fingerprint).await {
            Ok(run_id) => Some(run_id),
            Err(error) => {
                tracing::error!("failed to start Retrieval Run recording: {error}");
                None
            }
        };
    let mut completion = RetrievalRunCompletion {
        parser_version: Some(PARSER_VERSION.to_owned()),
        chunker_version: Some(CHUNKER_VERSION.to_owned()),
        ranking_profile: Some(RANKING_CONFIG_VERSION.to_owned()),
        ..RetrievalRunCompletion::default()
    };
    let mut failure_stage = "activation_state";

    const ACTIVATION_DEADLINE: Duration = Duration::from_secs(60);
    let (mut result, deadline_expired) = match tokio::time::timeout(ACTIVATION_DEADLINE, async {
        let previous_state = activation::decode_activation_state(request.state.as_deref())?;

        failure_stage = "index_head";
        let started = Instant::now();
        let _query_guard = state.inner.search_lock.lock().await;
        // Pin the active storage location until every index query completes;
        // a concurrent storage move cannot remove the selected SQLite file.
        let _storage_guard = state.inner.storage_access.read().await;
        // Preserve the public pre-index contract. A Project without a synced
        // Ref or usable installed generation must report that condition
        // before storage/index creation or model scheduling can mask it.
        let cache = super::commit_sync::memory_cache_under_storage_guard(
            state,
            DaemonMemoryCacheRequest {
                project_id: project_id.clone(),
            },
        )
        .await?;
        require_ready_memory_cache(cache, &project_id)?;
        let (pool, _storage) = active_project_index(state, &project_id).await?;
        let Some(revision_id) = index::ready_index_revision(state, &pool, &project_id).await?
        else {
            pool.close().await;
            drop(_storage_guard);
            scheduler::ensure_project_queued(state, &project_id).await?;
            return match state.inner.search_models.status() {
                SearchModelRuntimeStatus::Missing => Err(SearchFailure::model_preparing(
                    "search models are waiting for background preparation",
                )
                .into()),
                SearchModelRuntimeStatus::Preparing {
                    downloaded_bytes,
                    total_bytes,
                } => Err(SearchFailure::model_preparing(format!(
                    "search models are preparing ({downloaded_bytes}/{total_bytes} bytes)"
                ))
                .into()),
                SearchModelRuntimeStatus::Failed => Err(SearchFailure::model(
                    "search model preparation failed and will retry in the background",
                )
                .into()),
                SearchModelRuntimeStatus::Ready => {
                    let job = scheduler::status(state, &project_id).await?;
                    if let Some(job) = job
                        && job.state == "failed"
                    {
                        Err(SearchFailure::failed(job.last_error.unwrap_or_else(|| {
                            "the background search index build failed".to_owned()
                        }))
                        .into())
                    } else {
                        Err(SearchFailure::index_preparing(
                            "the first search index is building in the background; retry shortly",
                        )
                        .into())
                    }
                }
            };
        };
        completion.latencies.index_ensure_us = elapsed_us(started);
        completion.index_revision = Some(revision_id.clone());
        let (indexed_effective_hash, indexed_resources) =
            indexed_corpus(&pool, &revision_id).await?;
        completion.effective_hash = Some(indexed_effective_hash);
        completion.resources = indexed_resources;
        completion.latencies.effective_memory_us = elapsed_us(started);

        let response = query::query_index(
            state,
            &pool,
            &revision_id,
            &query,
            previous_state,
            &mut completion,
            &mut failure_stage,
        )
        .await;
        pool.close().await;
        response
    })
    .await
    {
        Ok(result) => (result, false),
        Err(_) => (
            Err(DaemonError::Search {
                code: "activation_deadline".to_owned(),
                message: format!(
                    "activate exceeded the {ACTIVATION_DEADLINE:?} budget; the Retrieval Run was finalized and remaining stages were skipped"
                ),
            }),
            true,
        ),
    };

    if let Ok(response) = &mut result {
        response.run_id = run_id.clone();
    }
    completion.latencies.total_us = elapsed_us(total_started);
    match &result {
        Ok(response) => {
            completion.returned_fragment_count = response.fragments.len();
        }
        Err(error) => {
            let (code, summary) = retrieval_error(error);
            completion.error_stage = Some(failure_stage.to_owned());
            completion.error_code = Some(code);
            completion.error_summary = Some(summary);
        }
    }
    if let Some(run_id) = run_id {
        let finish_result = if deadline_expired {
            // Keep the deadline terminalization independent of the history
            // lock and blob persistence. The response cannot be held after
            // the retrieval budget by another, unrelated history write.
            super::retrieval_history::finish_deadline_run(
                state,
                &run_id,
                failure_stage,
                completion.latencies.total_us,
            )
            .await
        } else {
            super::retrieval_history::finish_run(state, &run_id, completion).await
        };
        if let Err(error) = finish_result {
            tracing::error!("failed to finish Retrieval Run {run_id}: {error}");
            if let Err(record_error) =
                super::retrieval_history::record_persistence_failure(state, &run_id, &error).await
            {
                tracing::error!(
                    "failed to record Retrieval Run persistence failure {run_id}: {record_error}"
                );
            }
        }
    }
    result
}

async fn indexed_corpus(
    pool: &SqlitePool,
    revision_id: &str,
) -> Result<(String, Vec<RetrievalCorpusResourceInput>), DaemonError> {
    let effective_hash: String = sqlx::query_scalar(
        "SELECT effective_hash FROM search_revisions
         WHERE revision_id = $1 AND status = 'ready'",
    )
    .bind(revision_id)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query(
        "SELECT resource_id, scope, kind, path, title, content, content_hash,
                source_commit_id, draft_id, draft_revision
         FROM search_resources WHERE revision_id = $1 ORDER BY resource_id",
    )
    .bind(revision_id)
    .fetch_all(pool)
    .await?;
    let resources = rows
        .into_iter()
        .map(|row| {
            let scope = parse_source_scope(&row.try_get::<String, _>("scope")?)?;
            let kind_value: String = row.try_get("kind")?;
            let kind = parse_memory_kind(&kind_value).ok_or_else(|| {
                SearchFailure::failed(format!("unknown indexed memory kind: {kind_value}"))
            })?;
            Ok(RetrievalCorpusResourceInput {
                resource_id: row.try_get("resource_id")?,
                scope,
                kind,
                path: row.try_get("path")?,
                title: row.try_get("title")?,
                content: row.try_get("content")?,
                content_hash: row.try_get("content_hash")?,
                source_commit_id: row.try_get("source_commit_id")?,
                draft_id: row.try_get("draft_id")?,
                draft_revision: row.try_get("draft_revision")?,
            })
        })
        .collect::<Result<Vec<_>, DaemonError>>()?;
    Ok((effective_hash, resources))
}

fn retrieval_error(error: &DaemonError) -> (String, String) {
    let code = match error {
        DaemonError::Search { code, .. } => code.clone(),
        DaemonError::State { code, .. } => (*code).to_owned(),
        DaemonError::InvalidRequest(_) => "invalid_request".to_owned(),
        DaemonError::NotFound(_) => "not_found".to_owned(),
        DaemonError::Io(_) => "io_error".to_owned(),
        DaemonError::Sqlx(_) => "database_error".to_owned(),
        DaemonError::SerdeJson(_) => "serialization_error".to_owned(),
        DaemonError::Reqwest(_) | DaemonError::Server(_) | DaemonError::ServerResponse { .. } => {
            "server_error".to_owned()
        }
        DaemonError::CredentialStore(_) => "credential_store_error".to_owned(),
        DaemonError::InvalidConfig(_) => "invalid_config".to_owned(),
        DaemonError::Launchctl(_) => "launchctl_error".to_owned(),
        DaemonError::Ipc(_) => "ipc_error".to_owned(),
        DaemonError::Remote(error) => error.code.clone(),
    };
    (code, error.to_string())
}

pub(crate) async fn load_memory(
    state: &DaemonState,
    request: LoadMemoryRequest,
) -> Result<LoadMemoryResponse, DaemonError> {
    if request.project_id.trim().is_empty() || request.ids.is_empty() {
        return Err(DaemonError::InvalidRequest(
            "project_id and at least one memory id or path are required".to_owned(),
        ));
    }
    let effective = load_effective_memory(state, &request.project_id).await?;
    let by_id = effective
        .resources
        .iter()
        .map(|resource| (resource.resource_id.as_str(), resource))
        .collect::<HashMap<_, _>>();
    let by_path = effective
        .resources
        .iter()
        .map(|resource| (resource.path.as_str(), resource))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut resources = Vec::new();
    for requested in &request.ids {
        let resource = by_id
            .get(requested.as_str())
            .or_else(|| by_path.get(requested.as_str()))
            .copied()
            .ok_or_else(|| {
                SearchFailure::resource_not_found(format!(
                    "memory resource {requested} is not present in the current effective view"
                ))
            })?;
        if !seen.insert(resource.resource_id.clone()) {
            continue;
        }
        let known_hash = request
            .known_hashes
            .get(&resource.resource_id)
            .or_else(|| request.known_hashes.get(requested));
        let changed = known_hash != Some(&resource.content_hash);
        resources.push(LoadedMemoryResource {
            resource_id: resource.resource_id.clone(),
            scope: resource.scope,
            kind: resource.kind,
            path: resource.path.clone(),
            title: resource.title.clone(),
            description: String::new(),
            content_hash: resource.content_hash.clone(),
            changed,
            content: changed.then(|| resource.content.clone()),
        });
    }
    Ok(LoadMemoryResponse { resources })
}

pub(crate) async fn search_index_status(
    state: &DaemonState,
    request: SearchIndexProjectRequest,
) -> Result<SearchIndexStatus, DaemonError> {
    // Pin the active storage location until every project-index read has
    // completed. A storage move takes the write side before switching or
    // cleaning the source tree.
    let _storage_guard = state.inner.storage_access.read().await;
    let effective = load_effective_memory_under_storage_guard(state, &request.project_id).await?;
    let (pool, _storage) = active_project_index(state, &request.project_id).await?;
    let row = sqlx::query(
        "SELECT r.revision_id, r.effective_hash, r.status, r.last_error
         FROM search_heads h
         JOIN search_revisions r ON r.revision_id = h.revision_id
         WHERE h.project_id = $1",
    )
    .bind(&request.project_id)
    .fetch_optional(&pool)
    .await?;
    let (active_revision, active_effective_hash, revision_ready, last_error) = match row {
        Some(row) => {
            let status: String = row.try_get("status")?;
            (
                Some(row.try_get("revision_id")?),
                Some(row.try_get("effective_hash")?),
                status == "ready",
                row.try_get("last_error")?,
            )
        }
        None => (None, None, false, None),
    };
    let compatible_revision =
        index::ready_index_revision(state, &pool, &request.project_id).await?;
    let (model_status, model_downloaded_bytes, model_total_bytes) =
        match state.inner.search_models.status() {
            SearchModelRuntimeStatus::Missing => (SearchModelStatus::Missing, None, None),
            SearchModelRuntimeStatus::Preparing {
                downloaded_bytes,
                total_bytes,
            } => (
                SearchModelStatus::Preparing,
                Some(downloaded_bytes),
                Some(total_bytes),
            ),
            SearchModelRuntimeStatus::Ready => (SearchModelStatus::Ready, None, None),
            SearchModelRuntimeStatus::Failed => (SearchModelStatus::Failed, None, None),
        };
    let current_failure: Option<String> = sqlx::query_scalar(
        "SELECT last_error
         FROM search_revisions
         WHERE project_id = $1 AND effective_hash = $2 AND status = 'failed'
         ORDER BY created_at DESC
         LIMIT 1",
    )
    .bind(&request.project_id)
    .bind(&effective.effective_hash)
    .fetch_optional(&pool)
    .await?
    .flatten();
    let ready = revision_ready
        && active_effective_hash.as_deref() == Some(effective.effective_hash.as_str())
        && compatible_revision.as_deref() == active_revision.as_deref();
    let job = scheduler::status(state, &request.project_id).await?;
    let status = SearchIndexStatus {
        project_id: request.project_id,
        effective_hash: effective.effective_hash,
        active_revision,
        active_effective_hash,
        ready,
        model_status,
        model_downloaded_bytes,
        model_total_bytes,
        build_state: job.as_ref().map(|job| job.state.clone()),
        desired_sequence: job.as_ref().map(|job| job.desired_sequence),
        completed_sequence: job.as_ref().map(|job| job.completed_sequence),
        last_error: job
            .and_then(|job| job.last_error)
            .or(current_failure)
            .or(last_error),
    };
    pool.close().await;
    Ok(status)
}

pub(crate) async fn rebuild_search_index(
    state: &DaemonState,
    request: SearchIndexProjectRequest,
) -> Result<SearchIndexStatus, DaemonError> {
    scheduler::enqueue_project(state, &request.project_id).await?;
    search_index_status(state, request).await
}

pub(crate) async fn load_effective_memory(
    state: &DaemonState,
    project_id: &str,
) -> Result<EffectiveMemory, DaemonError> {
    let _storage_guard = state.inner.storage_access.read().await;
    load_effective_memory_under_storage_guard(state, project_id).await
}

async fn load_effective_memory_under_storage_guard(
    state: &DaemonState,
    project_id: &str,
) -> Result<EffectiveMemory, DaemonError> {
    let cache = super::commit_sync::memory_cache_under_storage_guard(
        state,
        DaemonMemoryCacheRequest {
            project_id: project_id.to_owned(),
        },
    )
    .await?;
    let cache = require_ready_memory_cache(cache, project_id)?;
    let _active_storage = super::project_storage::resolve_active(state, project_id).await?;

    let mut resources = BTreeMap::<String, EffectiveResource>::new();
    let mut adaptations = HashMap::<String, String>::new();
    if let (Some(root_path), Some(base_commit_id)) = (
        cache.active_generation_path.as_deref(),
        cache.commit_id.as_deref(),
    ) {
        let marker_path = PathBuf::from(root_path).join("commit-payload.json");
        let payload: overlay::CachedCommitPayload =
            serde_json::from_slice(&std::fs::read(&marker_path).map_err(|error| {
                SearchFailure::failed(format!(
                    "failed to read installed Commit payload {}: {error}",
                    marker_path.display()
                ))
            })?)
            .map_err(|error| {
                SearchFailure::failed(format!(
                    "installed Commit payload {} is invalid: {error}",
                    marker_path.display()
                ))
            })?;
        if payload.commit.commit_id != base_commit_id {
            return Err(SearchFailure::generation_changed(
                "installed Commit payload does not match the current Project Ref",
            )
            .into());
        }
        let blobs = payload
            .blobs
            .into_iter()
            .map(|blob| (blob.blob_id, blob.content))
            .collect::<HashMap<_, _>>();
        for entry in payload.tree.entries {
            if let Some(source) = &entry.org_source {
                adaptations.insert(entry.id.clone(), source.resource_id.clone());
            }
            let Some(kind) = overlay::cached_memory_kind(entry.kind) else {
                continue;
            };
            let Some(scope) = overlay::cached_scope(entry.scope) else {
                continue;
            };
            let path = entry.path.ok_or_else(|| {
                SearchFailure::failed(format!(
                    "memory Tree entry {} is missing its path",
                    entry.id
                ))
            })?;
            let blob = blobs.get(&entry.blob_id).ok_or_else(|| {
                SearchFailure::failed(format!(
                    "memory Tree entry {} references missing Blob {}",
                    entry.id, entry.blob_id
                ))
            })?;
            let (content, title) = project_authority_content(kind, &path, blob)?;
            resources.insert(
                entry.id.clone(),
                EffectiveResource {
                    source: SourceResource {
                        resource_id: entry.id,
                        project_id: project_id.to_owned(),
                        scope,
                        kind,
                        path,
                        title,
                        description: entry.description,
                        content_hash: sha256(&content),
                        content,
                        source_commit_id: Some(base_commit_id.to_owned()),
                        draft_id: None,
                        draft_revision: None,
                    },
                },
            );
        }
    }

    for draft in overlay::load_draft_overlays(&state.inner.pool, project_id).await? {
        for (_, _, operation) in &draft.operations {
            if let Some(create) = &operation.create
                && let Some(source) = &create.content.org_source
            {
                adaptations.insert(draft.draft_id.clone(), source.resource_id.clone());
            }
        }
        overlay::apply_draft_overlay(project_id, &mut resources, draft)?;
    }
    let adapted_sources = adaptations
        .into_iter()
        .filter_map(|(id, source)| resources.contains_key(&id).then_some(source))
        .collect::<std::collections::HashSet<_>>();
    resources.retain(|id, resource| {
        resource.source.scope != SourceScope::Org || !adapted_sources.contains(id)
    });
    let source_resources = resources
        .into_values()
        .map(|resource| resource.source)
        .collect::<Vec<_>>();
    let effective_hash =
        effective_memory_hash(project_id, cache.commit_id.as_deref(), &source_resources);
    Ok(EffectiveMemory {
        project_id: project_id.to_owned(),
        effective_hash,
        resources: source_resources.into(),
    })
}

fn require_ready_memory_cache(
    cache: DaemonMemoryCacheStatus,
    project_id: &str,
) -> Result<DaemonMemoryCacheStatus, DaemonError> {
    match cache.state {
        DaemonMemoryCacheState::Ready => Ok(cache),
        DaemonMemoryCacheState::ProjectRefNotSynced => Err(DaemonError::State {
            code: "project_ref_not_synced",
            message: cache.diagnostic.unwrap_or_else(|| {
                format!("the Project Ref for {project_id} has not been synchronized")
            }),
        }),
        DaemonMemoryCacheState::StorageUnavailable => Err(DaemonError::State {
            code: "project_storage_unavailable",
            message: cache
                .diagnostic
                .unwrap_or_else(|| format!("Project storage for {project_id} is unavailable")),
        }),
        DaemonMemoryCacheState::GenerationMissing => Err(DaemonError::State {
            code: "commit_generation_missing",
            message: cache
                .diagnostic
                .unwrap_or_else(|| format!("the Commit generation for {project_id} is missing")),
        }),
        DaemonMemoryCacheState::GenerationCorrupt => Err(DaemonError::State {
            code: "commit_generation_corrupt",
            message: cache
                .diagnostic
                .unwrap_or_else(|| format!("the Commit generation for {project_id} is corrupt")),
        }),
    }
}

pub(super) fn project_authority_content(
    _kind: MemoryKind,
    path: &str,
    blob: &str,
) -> Result<(String, String), DaemonError> {
    if blob.trim().is_empty() {
        return Err(
            SearchFailure::failed(format!("Memory Blob at {path} has empty content")).into(),
        );
    }
    let title = markdown_title(blob).unwrap_or_else(|| title_from_path(path));
    Ok((blob.to_owned(), title))
}

pub(super) fn parse_source_scope(value: &str) -> Result<SourceScope, DaemonError> {
    match value {
        "org" => Ok(SourceScope::Org),
        "project" => Ok(SourceScope::Project),
        _ => Err(SearchFailure::failed(format!("unknown Draft scope: {value}")).into()),
    }
}

pub(super) fn parse_memory_kind(value: &str) -> Option<MemoryKind> {
    match value {
        // Legacy kind values from archived indexes stay readable; the
        // unified runtime only writes 'memory'.
        "context" | "rule" | "workflow" | "memory" => Some(MemoryKind::Memory),
        _ => None,
    }
}

pub(super) fn markdown_title(content: &str) -> Option<String> {
    let mut in_heading = false;
    let mut title = String::new();
    for event in Parser::new_ext(content, Options::all()) {
        match event {
            Event::Start(Tag::Heading { .. }) if title.is_empty() => in_heading = true,
            Event::Text(text) | Event::Code(text) if in_heading => {
                if !title.is_empty() {
                    title.push(' ');
                }
                title.push_str(&text);
            }
            Event::End(pulldown_cmark::TagEnd::Heading(_)) if in_heading => {
                let collapsed = title.split_whitespace().collect::<Vec<_>>().join(" ");
                return (!collapsed.is_empty()).then_some(collapsed);
            }
            _ => {}
        }
    }
    None
}

pub(super) fn title_from_path(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path))
        .replace(['_', '-'], " ")
}

fn effective_memory_hash(
    project_id: &str,
    base_commit_id: Option<&str>,
    resources: &[SourceResource],
) -> String {
    let mut ordered = resources.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
    let mut hasher = Sha256::new();
    hasher.update(project_id.as_bytes());
    hasher.update([0]);
    hasher.update(base_commit_id.unwrap_or_default().as_bytes());
    for resource in ordered {
        for value in [
            resource.resource_id.as_str(),
            resource.kind.as_str(),
            resource.scope.as_str(),
            resource.path.as_str(),
            resource.content_hash.as_str(),
            resource.draft_id.as_deref().unwrap_or_default(),
            resource.draft_revision.as_deref().unwrap_or_default(),
        ] {
            hasher.update(value.as_bytes());
            hasher.update([0]);
        }
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub(super) fn sha256(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub(super) async fn run_model_work<T: Send + 'static>(
    state: &DaemonState,
    operation: impl FnOnce() -> Result<T, SearchFailure> + Send + 'static,
) -> Result<T, DaemonError> {
    let permit = state
        .inner
        .search_model_gate
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| SearchFailure::failed("search model inference gate closed"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
    .await
    .map_err(|error| {
        SearchFailure::failed(format!(
            "search model worker terminated unexpectedly: {error}"
        ))
    })?
    .map_err(DaemonError::from)
}

pub(super) fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::{
        ClearRetrievalRunsRequest, CreateEvaluationCaseRequest, CredentialStore,
        CredentialStoreError, DaemonContentDraftUpdate, DaemonCreateDraftOperation,
        DaemonDeleteDraftOperation, DaemonDraftOperationRecordSource, DaemonDraftOperationRequest,
        DaemonDraftOperationResponse, DaemonDraftOperationSource, DaemonIpcRequest,
        DaemonIpcService, DaemonProjectCacheClearRequest, DaemonProjectStorageRequest,
        DaemonRenameDraftOperation, DaemonUpdateDraftOperation, EvaluationCaseStatus,
        EvaluationEvidenceInput, ExportEvaluationSetRequest, ResolveEvaluationCaseRequest,
        RetrievalRunListRequest, RetrievalRunRequest, RetrievalRunStatus, ServerCredentials,
    };

    struct NoCredentials;

    impl CredentialStore for NoCredentials {
        fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
            Ok(None)
        }

        fn replace(&self, _credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
            Ok(())
        }

        fn clear(&self) -> Result<(), CredentialStoreError> {
            Ok(())
        }
    }

    struct DeterministicModels;

    struct FailingIndexModels;

    struct PreparingModels;

    struct BatchRecordingModels {
        largest_batch: AtomicUsize,
        total_embedded: AtomicUsize,
    }

    impl SearchModels for DeterministicModels {
        fn revision(&self) -> Result<String, SearchFailure> {
            Ok("deterministic-models.v1".to_owned())
        }

        fn token_offsets(&self, text: &str) -> Result<Vec<(usize, usize)>, SearchFailure> {
            Ok(text
                .char_indices()
                .map(|(start, character)| (start, start + character.len_utf8()))
                .collect())
        }

        fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure> {
            Ok(texts.iter().map(|text| test_vector(text)).collect())
        }

        fn embed_query(&self, query: &str) -> Result<Vec<f32>, SearchFailure> {
            Ok(test_vector(query))
        }

        fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>, SearchFailure> {
            let needle = query.to_lowercase();
            Ok(documents
                .iter()
                .map(|document| {
                    if document.to_lowercase().contains(&needle) {
                        1.0
                    } else {
                        0.0
                    }
                })
                .collect())
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn status(&self) -> SearchModelRuntimeStatus {
            SearchModelRuntimeStatus::Ready
        }
    }

    impl SearchModels for FailingIndexModels {
        fn revision(&self) -> Result<String, SearchFailure> {
            Ok("failing-index-models.v1".to_owned())
        }

        fn token_offsets(&self, text: &str) -> Result<Vec<(usize, usize)>, SearchFailure> {
            Ok(text
                .char_indices()
                .map(|(start, character)| (start, start + character.len_utf8()))
                .collect())
        }

        fn embed_passages(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure> {
            Err(SearchFailure::model("deterministic index failure"))
        }

        fn embed_query(&self, _query: &str) -> Result<Vec<f32>, SearchFailure> {
            unreachable!("query embedding is not reached when index construction fails")
        }

        fn rerank(&self, _query: &str, _documents: &[String]) -> Result<Vec<f32>, SearchFailure> {
            unreachable!("reranking is not reached when index construction fails")
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn status(&self) -> SearchModelRuntimeStatus {
            SearchModelRuntimeStatus::Ready
        }
    }

    impl SearchModels for PreparingModels {
        fn revision(&self) -> Result<String, SearchFailure> {
            unreachable!("model revision must not block while preparation is active")
        }

        fn token_offsets(&self, _text: &str) -> Result<Vec<(usize, usize)>, SearchFailure> {
            unreachable!("tokenization must not start while preparation is active")
        }

        fn embed_passages(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure> {
            unreachable!("embedding must not start while preparation is active")
        }

        fn embed_query(&self, _query: &str) -> Result<Vec<f32>, SearchFailure> {
            unreachable!("embedding must not start while preparation is active")
        }

        fn rerank(&self, _query: &str, _documents: &[String]) -> Result<Vec<f32>, SearchFailure> {
            unreachable!("reranking must not start while preparation is active")
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn status(&self) -> SearchModelRuntimeStatus {
            SearchModelRuntimeStatus::Preparing {
                downloaded_bytes: 128,
                total_bytes: 512,
            }
        }
    }

    impl SearchModels for BatchRecordingModels {
        fn revision(&self) -> Result<String, SearchFailure> {
            Ok("batch-recording-models.v1".to_owned())
        }

        fn token_offsets(&self, text: &str) -> Result<Vec<(usize, usize)>, SearchFailure> {
            Ok(text
                .char_indices()
                .map(|(start, character)| (start, start + character.len_utf8()))
                .collect())
        }

        fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure> {
            self.largest_batch.fetch_max(texts.len(), Ordering::Relaxed);
            self.total_embedded
                .fetch_add(texts.len(), Ordering::Relaxed);
            Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0]).collect())
        }

        fn embed_query(&self, _query: &str) -> Result<Vec<f32>, SearchFailure> {
            Ok(vec![1.0, 0.0, 0.0])
        }

        fn rerank(&self, _query: &str, documents: &[String]) -> Result<Vec<f32>, SearchFailure> {
            Ok(vec![1.0; documents.len()])
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn status(&self) -> SearchModelRuntimeStatus {
            SearchModelRuntimeStatus::Ready
        }
    }

    fn test_vector(text: &str) -> Vec<f32> {
        let lower = text.to_lowercase();
        if lower.contains("hybrid") || lower.contains("混合") {
            vec![1.0, 0.0, 0.0]
        } else if lower.contains("workflow") {
            vec![0.0, 1.0, 0.0]
        } else {
            vec![0.0, 0.0, 1.0]
        }
    }

    fn ranked_test_row(
        unit_key: &str,
        resource_id: &str,
        start_byte: usize,
        end_byte: usize,
        token_count: usize,
        rerank_score: f32,
    ) -> query::RankedRow {
        query::RankedRow {
            row: query::IndexRow {
                rowid: 1,
                unit_key: unit_key.to_owned(),
                resource_id: resource_id.to_owned(),
                scope: SourceScope::Project,
                kind: MemoryKind::Memory,
                path: format!("context/{resource_id}.md"),
                title: resource_id.to_owned(),
                heading_path: Vec::new(),
                locator: SourceLocator::MarkdownSpan {
                    start_byte,
                    end_byte,
                    heading_path: Vec::new(),
                },
                text: unit_key.to_owned(),
                text_hash: sha256(unit_key),
                resource_content_hash: sha256(unit_key),
                token_count,
                vector: vec![1.0, 0.0, 0.0],
            },
            exact_rank: None,
            bm25_rank: None,
            bm25_score: None,
            vector_rank: None,
            vector_score: None,
            rrf_rank: 1,
            rrf_score: 0.01,
            reranker_rank: Some(1),
            rerank_score: Some(rerank_score),
            final_rank: None,
            exclusion_reason: RetrievalExclusionReason::NotReranked,
            delta_action: None,
        }
    }

    async fn test_state() -> (TempDir, DaemonState) {
        let pair = test_state_with_models(Arc::new(DeterministicModels)).await;
        let _worker = pair.1.start_search_index_worker();
        scheduler::enqueue_project(&pair.1, "prj_test")
            .await
            .unwrap();
        wait_for_index_job(&pair.1, "ready").await;
        pair
    }

    async fn wait_for_index_job(state: &DaemonState, expected_state: &str) {
        for _ in 0..200 {
            if scheduler::status(state, "prj_test")
                .await
                .unwrap()
                .is_some_and(|job| job.state == expected_state)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let status = scheduler::status(state, "prj_test").await.unwrap();
        panic!("search index job did not reach {expected_state}: {status:?}");
    }

    #[test]
    fn activation_response_run_id_preserves_wire_compatibility() {
        let legacy = json!({
            "index_revision": "idx_1",
            "profile": "ranking.v1",
            "next_state": "state_1",
            "fragments": [],
            "removed": []
        });
        let mut response: ActivateMemoryResponse = serde_json::from_value(legacy).unwrap();
        assert_eq!(response.run_id, None);
        assert!(
            serde_json::to_value(&response)
                .unwrap()
                .get("run_id")
                .is_none()
        );

        response.run_id = Some("run_exact".to_owned());
        assert_eq!(
            serde_json::to_value(response).unwrap()["run_id"],
            "run_exact"
        );
    }

    #[tokio::test]
    async fn index_pruning_uses_search_then_storage_lock_order() {
        let (_temp, state) = test_state().await;
        let search_guard = state.inner.search_lock.lock().await;
        let prune_state = state.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let prune = tokio::spawn(async move {
            let _ = started_tx.send(());
            scheduler::prune_old_revisions(&prune_state, "prj_test").await
        });
        started_rx.await.unwrap();

        // If pruning held storage before waiting for search, this write lock
        // would deadlock with the search guard held by this task.
        let storage_guard = tokio::time::timeout(
            Duration::from_millis(100),
            state.inner.storage_access.write(),
        )
        .await
        .expect("pruning must not acquire storage before the search lock");
        drop(storage_guard);
        drop(search_guard);
        tokio::time::timeout(Duration::from_secs(1), prune)
            .await
            .expect("pruning did not resume after both locks became available")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn cache_clear_invalidates_a_claimed_build_before_publication() {
        let (_temp, state) = test_state_with_models(Arc::new(DeterministicModels)).await;
        scheduler::enqueue_project(&state, "prj_test")
            .await
            .unwrap();
        let claimed_sequence: i64 = sqlx::query_scalar(
            "UPDATE search_index_jobs
             SET state = 'building', building_sequence = desired_sequence
             WHERE project_id = 'prj_test'
             RETURNING desired_sequence",
        )
        .fetch_one(&state.inner.pool)
        .await
        .unwrap();

        let storage = state
            .project_storage(DaemonProjectStorageRequest {
                project_id: "prj_test".to_owned(),
            })
            .await
            .unwrap();
        state
            .clear_project_cache(DaemonProjectCacheClearRequest {
                project_id: "prj_test".to_owned(),
                expected_location_revision: storage.location_revision,
            })
            .await
            .unwrap();

        let status = scheduler::status(&state, "prj_test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.desired_sequence, claimed_sequence + 1);
        assert_eq!(status.completed_sequence, 0);
        assert_eq!(status.state, "queued");
        let stale_publish_cas = sqlx::query(
            "UPDATE search_index_jobs SET updated_at = updated_at
             WHERE project_id = 'prj_test' AND desired_sequence = $1
               AND building_sequence = $1 AND state = 'building'",
        )
        .bind(claimed_sequence)
        .execute(&state.inner.pool)
        .await
        .unwrap()
        .rows_affected();
        assert_eq!(stale_publish_cas, 0);
        let published_heads: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM search_heads WHERE project_id = 'prj_test'")
                .fetch_one(&state.inner.pool)
                .await
                .unwrap();
        assert_eq!(published_heads, 0);
    }

    #[tokio::test]
    async fn status_does_not_regress_a_newer_scheduler_publication() {
        let (_temp, state) = test_state().await;
        let (pool, _storage) = active_project_index(&state, "prj_test").await.unwrap();
        let observed_revision: String = sqlx::query_scalar(
            "SELECT revision_id FROM search_heads WHERE project_id = 'prj_test'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let newer_revision = "search_status_race_newer";
        sqlx::query(
            "INSERT INTO search_revisions (
                revision_id, project_id, effective_hash, model_revision,
                embedding_revision, parser_version, chunker_version,
                ranking_version, status, ready_at
             )
             SELECT $1, project_id, effective_hash, model_revision,
                    embedding_revision, parser_version, chunker_version,
                    ranking_version, 'ready', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             FROM search_revisions WHERE revision_id = $2",
        )
        .bind(newer_revision)
        .bind(&observed_revision)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE search_heads SET revision_id = $2 WHERE project_id = $1")
            .bind("prj_test")
            .bind(newer_revision)
            .execute(&pool)
            .await
            .unwrap();
        let effective_hash: String = sqlx::query_scalar(
            "SELECT effective_hash FROM search_revisions WHERE revision_id = $1",
        )
        .bind(newer_revision)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE search_heads
             SET revision_id = $2, effective_hash = $3
             WHERE project_id = $1",
        )
        .bind("prj_test")
        .bind(newer_revision)
        .bind(&effective_hash)
        .execute(&state.inner.pool)
        .await
        .unwrap();
        pool.close().await;

        // Resume a status request that had conceptually observed the older
        // revision before the scheduler published the newer one. Status is
        // read-only and therefore cannot write that stale observation back.
        let status = search_index_status(
            &state,
            SearchIndexProjectRequest {
                project_id: "prj_test".to_owned(),
            },
        )
        .await
        .unwrap();
        assert_eq!(status.active_revision.as_deref(), Some(newer_revision));
        let central_revision: String = sqlx::query_scalar(
            "SELECT revision_id FROM search_heads WHERE project_id = 'prj_test'",
        )
        .fetch_one(&state.inner.pool)
        .await
        .unwrap();
        assert_ne!(central_revision, observed_revision);
        assert_eq!(central_revision, newer_revision);
    }

    async fn test_state_with_models(
        search_models: Arc<dyn SearchModels>,
    ) -> (TempDir, DaemonState) {
        let temp = tempfile::tempdir().unwrap();
        let mut config = crate::DaemonConfig::for_root(temp.path().join("daemon"));
        config.project.server_url = "https://clumsies.test".to_owned();
        config.project.project_id = Some("prj_test".to_owned());
        let state = DaemonState::initialize_with_credential_store_and_search_models(
            config,
            Arc::new(NoCredentials),
            search_models,
        )
        .await
        .unwrap();

        let payload = json!({
            "commit": {
                "commit_id": "commit_test",
                "scope": "project",
                "org_id": "org_test",
                "project_id": "prj_test",
                "tree_id": "tree_test",
                "parent_commit_id": null,
                "version": 1,
                "created_at": "2026-07-21T00:00:00Z"
            },
            "tree": {
                "tree_id": "tree_test",
                "entries": [
                    {
                        "id": "ctx_retrieval",
                        "type": "context",
                        "scope": "project",
                        "project_id": "prj_test",
                        "path": "architecture/retrieval.md",
                        "blob_id": "blob_context",
                        "source": "project"
                    },
                    {
                        "id": "rule_testing",
                        "type": "rule",
                        "scope": "org",
                        "project_id": null,
                        "path": "coding/TESTING.md",
                        "blob_id": "blob_rule",
                        "source": "selected_org"
                    },
                    {
                        "id": "workflow_coding",
                        "type": "workflow",
                        "scope": "project",
                        "project_id": "prj_test",
                        "path": "workflow/CODING.md",
                        "blob_id": "blob_workflow",
                        "source": "project"
                    },
                ]
            },
            "blobs": [
                {
                    "blob_id": "blob_context",
                    "content": "# Retrieval\n\nHybrid search combines BM25 and dense vectors."
                },
                {
                    "blob_id": "blob_rule",
                    "content": "# Testing\n\nApply when changing retrieval.\n\nRun integration tests.\n\nTags: testing"
                },
                {
                    "blob_id": "blob_workflow",
                    "content": "# Coding Workflow\n\nImplement, test, and review."
                }
            ],
            "project_org_selection": null
        });
        let marker = serde_json::to_vec(&payload).unwrap();
        let storage = super::super::project_storage::resolve_active(&state, "prj_test")
            .await
            .unwrap();
        let root = storage.generation_path("commit_test");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("commit-payload.json"), marker).unwrap();
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&json!({
                "project_id": "prj_test",
                "commit_id": "commit_test",
                "tree_id": "tree_test",
                "ref_name": "refs/heads/main",
                "rules": {},
                "context": {}
            }))
            .unwrap(),
        )
        .unwrap();
        sqlx::query(
            "INSERT INTO cached_refs (
                ref_key, name, scope, org_id, project_id, commit_id, etag, server_updated_at
             ) VALUES ('project:prj_test', 'refs/heads/main', 'project', 'org_test',
                       'prj_test', 'commit_test', '\"commit_test\"', '2026-07-21T00:00:00Z')",
        )
        .execute(&state.inner.pool)
        .await
        .unwrap();
        for blob in payload["blobs"].as_array().unwrap() {
            sqlx::query("INSERT INTO cached_blobs (blob_id, content) VALUES ($1, $2)")
                .bind(blob["blob_id"].as_str().unwrap())
                .bind(blob["content"].as_str().unwrap())
                .execute(&state.inner.pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO cached_trees (tree_id, payload_json) VALUES ($1, $2)")
            .bind("tree_test")
            .bind(serde_json::to_string(&payload["tree"]).unwrap())
            .execute(&state.inner.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO cached_commits (
                commit_id, scope, org_id, project_id, tree_id,
                parent_commit_id, version, created_at, payload_json
             ) VALUES ($1, 'project', 'org_test', 'prj_test', $2, NULL, 1, $3, $4)",
        )
        .bind("commit_test")
        .bind("tree_test")
        .bind("2026-07-21T00:00:00Z")
        .bind(serde_json::to_string(&payload["commit"]).unwrap())
        .execute(&state.inner.pool)
        .await
        .unwrap();
        (temp, state)
    }

    #[tokio::test]
    async fn activation_deadline_terminalization_does_not_wait_for_history_serialization() {
        let (_temp, state) = test_state_with_models(Arc::new(DeterministicModels)).await;
        let run_id = crate::retrieval_history::start_run(
            &state,
            "prj_test",
            "deadline query",
            "fingerprint",
        )
        .await
        .unwrap();
        let _history_guard = state.inner.retrieval_history_lock.lock().await;

        tokio::time::timeout(
            Duration::from_millis(250),
            crate::retrieval_history::finish_deadline_run(&state, &run_id, "retrieval", 250_000),
        )
        .await
        .expect("deadline terminalization must not wait for the history lock")
        .unwrap();

        let row = sqlx::query(
            "SELECT status, error_stage, error_code, completed_at
             FROM retrieval_runs WHERE run_id = $1",
        )
        .bind(&run_id)
        .fetch_one(&state.inner.pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("status"), "failed");
        assert_eq!(row.get::<String, _>("error_stage"), "retrieval");
        assert_eq!(row.get::<String, _>("error_code"), "activation_deadline");
        assert!(row.get::<Option<String>, _>("completed_at").is_some());
    }

    #[tokio::test]
    async fn commit_activate_load_and_draft_delta_form_one_effective_memory_loop() {
        let (_temp, state) = test_state().await;
        let first = state
            .activate_memory(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "testing".to_owned(),
                state: None,
            })
            .await
            .unwrap();
        assert!(first.fragments.iter().any(|fragment| {
            fragment.resource_id == "rule_testing"
                && fragment.action == ActivationAction::Add
                && fragment
                    .content
                    .as_deref()
                    .is_some_and(|content| content.contains("integration tests"))
        }));
        let second = state
            .activate_memory(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "testing".to_owned(),
                state: Some(first.next_state.clone()),
            })
            .await
            .unwrap();
        let reused = second
            .fragments
            .iter()
            .find(|fragment| fragment.resource_id == "rule_testing")
            .unwrap();
        assert_eq!(reused.action, ActivationAction::Reuse);
        assert!(reused.content.is_none());

        state
            .store_draft_operation(DaemonDraftOperationRequest {
                draft_id: None,
                base_commit_id: Some("commit_test".to_owned()),
                project_id: "prj_test".to_owned(),
                scope: crate::DaemonDraftScope::Org,
                resource: crate::DaemonDraftResourceKind::Memory,
                op: DaemonDraftOperation {
                    create: None,
                    update: Some(DaemonUpdateDraftOperation::Content(
                        DaemonContentDraftUpdate {
                        id: "rule_testing".to_owned(),
                        content: DaemonDraftContent {
                            org_source: None,
                            description: None,
                            content: "# Testing\n\nTesting now covers BM25, vectors, RRF, and reranking."
                                .to_owned(),
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

        // Mutation scheduling is asynchronous: the old ready head stays
        // queryable while the replacement is built.
        let stale = state
            .activate_memory(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "testing".to_owned(),
                state: Some(second.next_state.clone()),
            })
            .await
            .unwrap();
        assert_eq!(stale.index_revision, first.index_revision);
        wait_for_index_job(&state, "ready").await;

        let third = state
            .activate_memory(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "testing".to_owned(),
                state: Some(second.next_state),
            })
            .await
            .unwrap();
        assert_ne!(third.index_revision, first.index_revision);
        let replaced = third
            .fragments
            .iter()
            .find(|fragment| fragment.resource_id == "rule_testing")
            .unwrap();
        assert_eq!(replaced.action, ActivationAction::Replace);
        assert!(
            replaced
                .content
                .as_deref()
                .is_some_and(|content| content.contains("RRF"))
        );

        let load_response = DaemonIpcService::new(state.clone())
            .dispatch(DaemonIpcRequest::new(
                "load_memory",
                serde_json::to_value(LoadMemoryRequest {
                    project_id: "prj_test".to_owned(),
                    ids: vec!["coding/TESTING.md".to_owned()],
                    known_hashes: BTreeMap::from([(
                        "rule_testing".to_owned(),
                        first
                            .fragments
                            .iter()
                            .find(|fragment| fragment.resource_id == "rule_testing")
                            .unwrap()
                            .content_hash
                            .clone(),
                    )]),
                })
                .unwrap(),
            ))
            .await;
        assert!(load_response.ok);
        let loaded: LoadMemoryResponse = serde_json::from_value(load_response.payload).unwrap();
        assert_eq!(loaded.resources.len(), 1);
        assert!(loaded.resources[0].changed);
        assert!(
            loaded.resources[0]
                .content
                .as_deref()
                .is_some_and(|content| content.contains("reranking"))
        );

        let (index_pool, _storage) = active_project_index(&state, "prj_test").await.unwrap();
        let indexed_kinds = sqlx::query_scalar::<_, String>(
            "SELECT kind FROM search_resources
             WHERE revision_id = $1 ORDER BY kind",
        )
        .bind(&third.index_revision)
        .fetch_all(&index_pool)
        .await
        .unwrap();
        index_pool.close().await;
        assert_eq!(indexed_kinds, ["memory", "memory", "memory"]);
    }

    #[tokio::test]
    async fn activation_trace_and_evaluation_case_share_one_ranked_result() {
        let (_temp, state) = test_state().await;
        let response = state
            .activate_memory(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "hybrid retrieval testing".to_owned(),
                state: None,
            })
            .await
            .unwrap();

        let runs = state
            .list_retrieval_runs(RetrievalRunListRequest {
                project_id: Some("prj_test".to_owned()),
                status: Some(RetrievalRunStatus::Succeeded),
                cursor: None,
                limit: Some(10),
            })
            .await
            .unwrap();
        assert_eq!(runs.items.len(), 1);
        let run = &runs.items[0];
        assert_eq!(response.run_id.as_deref(), Some(run.run_id.as_str()));
        assert_eq!(run.query, "hybrid retrieval testing");
        assert!(run.completed_at.is_some());
        assert_eq!(run.returned_fragment_count, response.fragments.len() as u64);
        assert_eq!(run.parser_version.as_deref(), Some(PARSER_VERSION));
        assert_eq!(run.chunker_version.as_deref(), Some(CHUNKER_VERSION));
        assert_eq!(run.ranking_profile.as_deref(), Some(RANKING_CONFIG_VERSION));

        let detail = state
            .get_retrieval_run(RetrievalRunRequest {
                run_id: run.run_id.clone(),
            })
            .await
            .unwrap();
        assert!(!detail.candidates.is_empty());
        let selected = detail
            .candidates
            .iter()
            .filter(|candidate| candidate.selected)
            .collect::<Vec<_>>();
        assert_eq!(selected.len(), response.fragments.len());
        for candidate in &detail.candidates {
            assert!(
                candidate.exact_rank.is_some()
                    || candidate.bm25_rank.is_some()
                    || candidate.vector_rank.is_some()
            );
            if candidate.selected {
                assert_eq!(
                    candidate.exclusion_reason,
                    RetrievalExclusionReason::Selected
                );
                let fragment = response
                    .fragments
                    .iter()
                    .find(|fragment| {
                        fragment.resource_id == candidate.resource_id
                            && fragment.heading_path == candidate.heading_path
                    })
                    .expect("selected trace candidate must be returned");
                let expected_action = match fragment.action {
                    ActivationAction::Add => RetrievalDeltaAction::Add,
                    ActivationAction::Replace => RetrievalDeltaAction::Replace,
                    ActivationAction::Reuse => RetrievalDeltaAction::Reuse,
                };
                assert_eq!(candidate.delta_action, Some(expected_action));
            } else {
                assert_ne!(
                    candidate.exclusion_reason,
                    RetrievalExclusionReason::Selected
                );
            }
        }

        sqlx::query(
            "INSERT INTO retrieval_run_candidates (
                run_id, candidate_order, unit_key, resource_id, scope, kind, path,
                heading_path_json, locator_json, content_hash, resource_content_hash,
                token_count, evidence_excerpt, exact_rank, bm25_rank, bm25_score,
                vector_rank, vector_score, rrf_rank, rrf_score, reranker_rank,
                reranker_logit, reranker_relevance, final_rank, selected,
                exclusion_reason, delta_action
             )
             SELECT run_id,
                    (SELECT COALESCE(MAX(candidate_order), -1) + 1
                     FROM retrieval_run_candidates WHERE run_id = $1),
                    unit_key || ':suggested', resource_id, scope, kind, path,
                    heading_path_json, locator_json, content_hash,
                    resource_content_hash, token_count,
                    'A relevant unit omitted from the assembled result.',
                    exact_rank, bm25_rank, bm25_score, vector_rank, vector_score,
                    rrf_rank + 10, rrf_score, reranker_rank + 10,
                    reranker_logit, reranker_relevance, NULL, 0,
                    'per_resource_limit', NULL
             FROM retrieval_run_candidates
             WHERE run_id = $1
             ORDER BY candidate_order
             LIMIT 1",
        )
        .bind(&run.run_id)
        .execute(&state.inner.pool)
        .await
        .unwrap();

        let evaluation = state
            .create_evaluation_case(CreateEvaluationCaseRequest {
                run_id: run.run_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            evaluation.evaluation_case.status,
            EvaluationCaseStatus::Draft
        );
        assert_eq!(evaluation.evaluation_case.version, 1);
        assert!(evaluation.evidence.is_empty());
        assert!(!evaluation.evidence_suggestions.is_empty());
        assert!(evaluation.report.is_none());

        let draft_export = state
            .export_evaluation_set(ExportEvaluationSetRequest {
                project_id: Some("prj_test".to_owned()),
                case_ids: vec![evaluation.evaluation_case.case_id.clone()],
            })
            .await
            .unwrap_err();
        assert!(matches!(draft_export, DaemonError::InvalidRequest(_)));

        let suggestion = &evaluation.evidence_suggestions[0];
        let resolved = state
            .resolve_evaluation_case(ResolveEvaluationCaseRequest {
                case_id: evaluation.evaluation_case.case_id.clone(),
                expected_version: 1,
                evidence: vec![EvaluationEvidenceInput {
                    resource_id: suggestion.resource_id.clone(),
                    unit_key: Some(suggestion.unit_key.clone()),
                }],
                none_matched: false,
            })
            .await
            .unwrap();
        assert_eq!(resolved.evaluation_case.status, EvaluationCaseStatus::Ready);
        assert_eq!(resolved.evaluation_case.version, 2);
        assert_eq!(resolved.evidence.len(), 1);
        assert!(resolved.report.is_some());

        let stale = state
            .resolve_evaluation_case(ResolveEvaluationCaseRequest {
                case_id: resolved.evaluation_case.case_id.clone(),
                expected_version: 1,
                evidence: Vec::new(),
                none_matched: true,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            stale,
            DaemonError::State {
                code: "evaluation_case_conflict",
                ..
            }
        ));

        let needs_evidence = state
            .resolve_evaluation_case(ResolveEvaluationCaseRequest {
                case_id: resolved.evaluation_case.case_id.clone(),
                expected_version: 2,
                evidence: Vec::new(),
                none_matched: true,
            })
            .await
            .unwrap();
        assert_eq!(
            needs_evidence.evaluation_case.status,
            EvaluationCaseStatus::NeedsEvidence
        );
        assert_eq!(needs_evidence.evaluation_case.version, 3);
        assert!(needs_evidence.evidence.is_empty());
        assert!(needs_evidence.report.is_none());

        let needs_evidence_export = state
            .export_evaluation_set(ExportEvaluationSetRequest {
                project_id: Some("prj_test".to_owned()),
                case_ids: vec![needs_evidence.evaluation_case.case_id.clone()],
            })
            .await
            .unwrap_err();
        assert!(matches!(
            needs_evidence_export,
            DaemonError::InvalidRequest(_)
        ));

        let resolved = state
            .resolve_evaluation_case(ResolveEvaluationCaseRequest {
                case_id: needs_evidence.evaluation_case.case_id.clone(),
                expected_version: 3,
                evidence: vec![EvaluationEvidenceInput {
                    resource_id: suggestion.resource_id.clone(),
                    unit_key: Some(suggestion.unit_key.clone()),
                }],
                none_matched: false,
            })
            .await
            .unwrap();
        assert_eq!(resolved.evaluation_case.version, 4);

        let exported = state
            .export_evaluation_set(ExportEvaluationSetRequest {
                project_id: Some("prj_test".to_owned()),
                case_ids: vec![resolved.evaluation_case.case_id.clone()],
            })
            .await
            .unwrap();
        let fixture: serde_json::Value = serde_json::from_str(&exported.fixture_json).unwrap();
        assert_eq!(fixture["version"], 2);
        assert_eq!(fixture["cases"].as_array().unwrap().len(), 1);
        assert_eq!(exported.report.variants.len(), 4);
        for variant in ["b1_bm25", "b2_dense_vector", "b3_hybrid_rrf", "b4_reranked"] {
            assert_eq!(exported.report.variants[variant].case_count, 1);
        }

        state
            .activate_memory(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "workflow".to_owned(),
                state: None,
            })
            .await
            .unwrap();
        let cleared = state
            .clear_retrieval_runs(ClearRetrievalRunsRequest {
                project_id: Some("prj_test".to_owned()),
            })
            .await
            .unwrap();
        assert_eq!(cleared.deleted_run_count, 1);
        let retained = state
            .list_retrieval_runs(RetrievalRunListRequest {
                project_id: Some("prj_test".to_owned()),
                status: None,
                cursor: None,
                limit: Some(10),
            })
            .await
            .unwrap();
        assert_eq!(retained.items.len(), 1);
        assert_eq!(retained.items[0].run_id, run.run_id);
        assert_eq!(
            retained.items[0].evaluation_case_id.as_deref(),
            Some(resolved.evaluation_case.case_id.as_str())
        );
        assert_eq!(
            retained.items[0].evaluation_case_status,
            Some(EvaluationCaseStatus::Ready)
        );
        let storage = state
            .project_storage(DaemonProjectStorageRequest {
                project_id: "prj_test".to_owned(),
            })
            .await
            .unwrap();
        state
            .clear_project_cache(DaemonProjectCacheClearRequest {
                project_id: "prj_test".to_owned(),
                expected_location_revision: storage.location_revision,
            })
            .await
            .unwrap();
        let after_cache_clear = state
            .get_retrieval_run(RetrievalRunRequest {
                run_id: run.run_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(after_cache_clear.evidence.len(), 1);
        assert!(after_cache_clear.report.is_some());
        let exported_after_cache_clear = state
            .export_evaluation_set(ExportEvaluationSetRequest {
                project_id: Some("prj_test".to_owned()),
                case_ids: vec![resolved.evaluation_case.case_id.clone()],
            })
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&exported_after_cache_clear.fixture_json)
                .unwrap()["cases"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let config = state.inner.config.clone();
        drop(state);
        let restarted = DaemonState::initialize_with_credential_store_and_search_models(
            config,
            Arc::new(NoCredentials),
            Arc::new(DeterministicModels),
        )
        .await
        .unwrap();
        let after_restart = restarted
            .list_retrieval_runs(RetrievalRunListRequest {
                project_id: Some("prj_test".to_owned()),
                status: None,
                cursor: None,
                limit: Some(10),
            })
            .await
            .unwrap();
        assert_eq!(after_restart.items, retained.items);
    }

    #[tokio::test]
    async fn retrieval_run_cursor_pagination_is_stable() {
        let (_temp, state) = test_state().await;
        for query in ["hybrid", "workflow", "testing"] {
            state
                .activate_memory(ActivateMemoryRequest {
                    project_id: "prj_test".to_owned(),
                    query: query.to_owned(),
                    state: None,
                })
                .await
                .unwrap();
        }

        let first = state
            .list_retrieval_runs(RetrievalRunListRequest {
                project_id: Some("prj_test".to_owned()),
                status: None,
                cursor: None,
                limit: Some(2),
            })
            .await
            .unwrap();
        assert_eq!(first.items.len(), 2);
        let second = state
            .list_retrieval_runs(RetrievalRunListRequest {
                project_id: Some("prj_test".to_owned()),
                status: None,
                cursor: first.next_cursor,
                limit: Some(2),
            })
            .await
            .unwrap();
        assert_eq!(second.items.len(), 1);
        assert!(second.next_cursor.is_none());
        assert!(
            first
                .items
                .iter()
                .all(|run| second.items.iter().all(|next| next.run_id != run.run_id))
        );
    }

    #[tokio::test]
    async fn mcp_text_replacements_are_atomic_and_persist_as_complete_content() {
        let (_temp, state) = test_state().await;
        sqlx::query(
            "INSERT INTO cached_refs (
                ref_key, name, scope, org_id, project_id, commit_id, etag, server_updated_at
             ) VALUES ('org:org_test', 'refs/heads/main', 'org', 'org_test', NULL,
                       'commit_test', '\"commit_test\"', '2026-07-21T00:00:00Z')",
        )
        .execute(&state.inner.pool)
        .await
        .unwrap();
        let central_pool = state.inner.pool.clone();
        let service = DaemonIpcService::new(state);
        let original = service
            .load_memory(LoadMemoryRequest {
                project_id: "prj_test".to_owned(),
                ids: vec!["rule_testing".to_owned()],
                known_hashes: BTreeMap::new(),
            })
            .await
            .unwrap()
            .resources
            .into_iter()
            .next()
            .unwrap();

        let response = service
            .dispatch(DaemonIpcRequest::new(
                "store_draft_operation",
                json!({
                    "project_id": "prj_test",
                    "scope": "project",
                    "resource": "memory",
                    "op": {
                        "update": {
                            "id": "rule_testing",
                            "expected_hash": original.content_hash,
                            "replacements": [
                                {
                                    "old_text": "Apply when changing retrieval.",
                                    "new_text": "Apply when changing retrieval behavior."
                                },
                                {
                                    "old_text": "Run integration tests.",
                                    "new_text": "Run integration and regression tests."
                                }
                            ]
                        }
                    },
                    "source": "mcp_store"
                }),
            ))
            .await;
        assert!(
            response.ok,
            "daemon rejected text replacement envelope: {:?}",
            response.error
        );
        let stored: DaemonDraftOperationResponse =
            serde_json::from_value(response.payload).unwrap();

        let loaded = service
            .load_memory(LoadMemoryRequest {
                project_id: "prj_test".to_owned(),
                ids: vec![stored.draft_id.clone()],
                known_hashes: BTreeMap::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            loaded.resources[0].content.as_deref(),
            Some(
                "# Testing\n\nApply when changing retrieval behavior.\n\nRun integration and regression tests.\n\nTags: testing"
            )
        );
        let updated_hash = loaded.resources[0].content_hash.clone();

        // Hold the central SQLite writer slot while the second mutation reads
        // and prepares its replacement. A deferred transaction would acquire
        // a stale WAL snapshot and then fail its read-to-write upgrade with
        // SQLITE_BUSY_SNAPSHOT (517). The mutation must instead wait for its
        // BEGIN IMMEDIATE reservation and commit normally.
        let writer = central_pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let repeated_service = service.clone();
        let adaptation_id = stored.draft_id.clone();
        let repeated_task = tokio::spawn(async move {
            repeated_service
                .dispatch(DaemonIpcRequest::new(
                    "store_draft_operation",
                    json!({
                        "project_id": "prj_test",
                        "scope": "project",
                        "resource": "memory",
                        "op": {
                            "update": {
                                "id": adaptation_id,
                                "expected_hash": updated_hash,
                                "replacements": [{
                                    "old_text": "Run integration and regression tests.",
                                    "new_text": "Run integration, regression, and smoke tests."
                                }]
                            }
                        },
                        "source": "mcp_store"
                    }),
                ))
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        writer.commit().await.unwrap();
        let repeated = repeated_task.await.unwrap();
        assert!(
            repeated.ok,
            "daemon rejected repeated text replacement: {:?}",
            repeated.error
        );
        let repeated: DaemonDraftOperationResponse =
            serde_json::from_value(repeated.payload).unwrap();
        assert_eq!(repeated.draft_id, stored.draft_id);

        let adaptation_id = stored.draft_id.clone();
        let stale = service
            .dispatch(DaemonIpcRequest::new(
                "store_draft_operation",
                json!({
                    "project_id": "prj_test",
                    "scope": "project",
                    "resource": "memory",
                    "op": {
                        "update": {
                            "id": adaptation_id,
                            "expected_hash": original.content_hash,
                            "replacements": [{
                                "old_text": "Run integration and regression tests.",
                                "new_text": "stale write"
                            }]
                        }
                    },
                    "source": "mcp_store"
                }),
            ))
            .await;
        assert!(!stale.ok);
        assert_eq!(
            stale.error.as_ref().map(|error| error.code.as_str()),
            Some("memory_content_changed")
        );

        let detail = service.get_draft(&stored.draft_id).await.unwrap();
        assert_eq!(detail.operations.len(), 2);
        assert_eq!(
            detail.operations[1].source,
            DaemonDraftOperationRecordSource::McpStore
        );
        let DaemonUpdateDraftOperation::Content(update) =
            detail.operations[1].operation.update.as_ref().unwrap()
        else {
            panic!("text replacement must be materialized before persistence");
        };
        assert_eq!(
            update.content,
            DaemonDraftContent {
                org_source: None,
                description: None,
                content: "# Testing\n\nApply when changing retrieval behavior.\n\nRun integration, regression, and smoke tests.\n\nTags: testing"
                    .to_owned()
            }
        );
    }

    #[tokio::test]
    async fn mcp_target_mutations_preserve_project_ownership() {
        let (_temp, state) = test_state().await;
        let pool = state.inner.pool.clone();
        let service = DaemonIpcService::new(state);
        let original = service
            .load_memory(LoadMemoryRequest {
                project_id: "prj_test".to_owned(),
                ids: vec!["ctx_retrieval".to_owned()],
                known_hashes: BTreeMap::new(),
            })
            .await
            .unwrap()
            .resources
            .into_iter()
            .next()
            .unwrap();

        let requests = [
            json!({
                "update": {
                    "id": "ctx_retrieval",
                    "expected_hash": original.content_hash,
                    "replacements": [{
                        "old_text": "Hybrid search",
                        "new_text": "Updated search"
                    }]
                }
            }),
            json!({
                "rename": {
                    "id": "ctx_retrieval",
                    "new_path": "architecture/renamed.md"
                }
            }),
            json!({
                "delete": {
                    "id": "ctx_retrieval"
                }
            }),
        ];
        for op in requests {
            let response = service
                .dispatch(DaemonIpcRequest::new(
                    "store_draft_operation",
                    json!({
                        "project_id": "prj_test",
                        "scope": "org",
                        "resource": "memory",
                        "op": op,
                        "source": "mcp_store"
                    }),
                ))
                .await;
            assert!(response.ok, "mutation failed: {:?}", response.error);
        }

        let draft_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_drafts")
            .fetch_one(&pool)
            .await
            .unwrap();
        let operation_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM local_draft_operations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(draft_count, 1);
        assert_eq!(operation_count, 3);
        let scope: String = sqlx::query_scalar("SELECT resource_scope FROM local_drafts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(scope, "project");
    }

    #[tokio::test]
    async fn mcp_discard_resolves_existing_legacy_draft_scope_without_fabricating_a_draft() {
        let (_temp, state) = test_state().await;
        let pool = state.inner.pool.clone();
        let service = DaemonIpcService::new(state);
        let draft_id = "draft_legacy_project";
        sqlx::query(
            "INSERT INTO local_drafts (
                draft_id, project_id, base_commit_id, current_commit_id,
                resource_scope, resource_kind, target_id, status
             ) VALUES ($1, 'prj_test', 'commit_test', 'commit_test',
                       'project', 'memory', 'ctx_retrieval', 'open')",
        )
        .bind(draft_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO local_draft_operations (
                local_operation_id, draft_id, resource_kind, operation_json,
                source, sync_status
             ) VALUES (
                'op_legacy_project', $1, 'memory',
                '{\"update\":{\"id\":\"ctx_retrieval\",\"content\":{\"description\":null,\"content\":\"# Legacy draft\\n\\nCleanup only.\"},\"description\":null}}',
                'server', 'synced'
             )",
        )
        .bind(draft_id)
        .execute(&pool)
        .await
        .unwrap();

        let discarded = service
            .dispatch(DaemonIpcRequest::new(
                "store_draft_operation",
                json!({
                    "project_id": "prj_test",
                    "scope": "org",
                    "resource": "memory",
                    "op": {"discard": {"id": draft_id}},
                    "source": "mcp_store"
                }),
            ))
            .await;
        assert!(discarded.ok, "discard failed: {:?}", discarded.error);
        let response: DaemonDraftOperationResponse =
            serde_json::from_value(discarded.payload).unwrap();
        assert_eq!(response.draft_id, draft_id);
        let detail = service.get_draft(draft_id).await.unwrap();
        assert_eq!(detail.draft.scope, crate::DaemonDraftScope::Project);
        assert_eq!(
            detail.draft.status,
            crate::DaemonLocalDraftStatus::Discarded
        );
        let draft_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_drafts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(draft_count, 1);

        let missing = service
            .dispatch(DaemonIpcRequest::new(
                "store_draft_operation",
                json!({
                    "project_id": "prj_test",
                    "scope": "org",
                    "resource": "memory",
                    "op": {"discard": {"id": "draft_missing"}},
                    "source": "mcp_store"
                }),
            ))
            .await;
        assert!(!missing.ok);
        assert_eq!(missing.error.unwrap().code, "not_found");
        let final_draft_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_drafts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(final_draft_count, 1);
    }

    #[tokio::test]
    async fn desktop_creates_project_authority_drafts() {
        let (_temp, state) = test_state().await;
        let response = state
            .store_draft_operation(DaemonDraftOperationRequest {
                draft_id: None,
                base_commit_id: Some("commit_test".to_owned()),
                project_id: "prj_test".to_owned(),
                scope: crate::DaemonDraftScope::Project,
                resource: crate::DaemonDraftResourceKind::Memory,
                op: DaemonDraftOperation {
                    create: Some(DaemonCreateDraftOperation {
                        path: "project/new.md".to_owned(),
                        content: DaemonDraftContent {
                            org_source: None,
                            description: Some("Project authority".to_owned()),
                            content: "# New".to_owned(),
                        },
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
        let detail = state.get_draft(&response.draft_id).await.unwrap();
        assert_eq!(detail.draft.scope, crate::DaemonDraftScope::Project);
        let draft_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_drafts")
            .fetch_one(&state.inner.pool)
            .await
            .unwrap();
        assert_eq!(draft_count, 1);
    }

    #[tokio::test]
    async fn pure_create_draft_path_only_rename_preserves_content_and_description() {
        let (_temp, state) = test_state().await;
        let service = DaemonIpcService::new(state.clone());
        let original_content = DaemonDraftContent {
            org_source: None,
            description: Some("Keep this semantic description.".to_owned()),
            content: "# New Memory\n\nOriginal body.".to_owned(),
        };
        let created = service
            .store_draft_operation(DaemonDraftOperationRequest {
                draft_id: None,
                base_commit_id: None,
                project_id: "prj_test".to_owned(),
                scope: crate::DaemonDraftScope::Org,
                resource: crate::DaemonDraftResourceKind::Memory,
                op: DaemonDraftOperation {
                    create: Some(DaemonCreateDraftOperation {
                        path: "guides/original.md".to_owned(),
                        content: original_content.clone(),
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

        let renamed = service
            .store_draft_operation(DaemonDraftOperationRequest {
                draft_id: Some(created.draft_id.clone()),
                base_commit_id: None,
                project_id: "prj_test".to_owned(),
                scope: crate::DaemonDraftScope::Org,
                resource: crate::DaemonDraftResourceKind::Memory,
                op: DaemonDraftOperation {
                    create: None,
                    update: None,
                    rename: Some(DaemonRenameDraftOperation {
                        id: created.draft_id.clone(),
                        new_path: "guides/renamed.md".to_owned(),
                        description: None,
                    }),
                    delete: None,
                    discard: None,
                },
                source: Some(DaemonDraftOperationSource::Desktop),
            })
            .await
            .unwrap();

        assert_eq!(renamed.draft_id, created.draft_id);
        let detail = service.get_draft(&created.draft_id).await.unwrap();
        assert_eq!(detail.operations.len(), 2);
        assert_eq!(
            detail.operations[0]
                .operation
                .create
                .as_ref()
                .unwrap()
                .content,
            original_content
        );

        let effective = load_effective_memory(&state, "prj_test").await.unwrap();
        let loaded = effective
            .resources
            .iter()
            .find(|resource| resource.resource_id == created.draft_id)
            .unwrap();
        assert_eq!(loaded.path, "guides/renamed.md");
        assert_eq!(loaded.description, "Keep this semantic description.");
        assert_eq!(loaded.content, "# New Memory\n\nOriginal body.");
    }

    #[tokio::test]
    async fn mcp_text_update_of_org_reference_creates_explicit_project_adaptation() {
        let (_temp, state) = test_state().await;
        sqlx::query(
            "INSERT INTO cached_refs (
                ref_key, name, scope, org_id, project_id, commit_id, etag, server_updated_at
             ) VALUES ('org:org_test', 'refs/heads/main', 'org', 'org_test', NULL,
                       'commit_test', '\"commit_test\"', '2026-07-21T00:00:00Z')",
        )
        .execute(&state.inner.pool)
        .await
        .unwrap();
        let service = DaemonIpcService::new(state);
        let original = service
            .load_memory(LoadMemoryRequest {
                project_id: "prj_test".to_owned(),
                ids: vec!["rule_testing".to_owned()],
                known_hashes: BTreeMap::new(),
            })
            .await
            .unwrap()
            .resources
            .into_iter()
            .next()
            .unwrap();

        let response = service
            .dispatch(DaemonIpcRequest::new(
                "store_draft_operation",
                json!({
                    "project_id": "prj_test",
                    "scope": "project",
                    "resource": "memory",
                    "op": {
                        "update": {
                            "id": "rule_testing",
                            "expected_hash": original.content_hash,
                            "replacements": [{
                                "old_text": "Run integration tests.",
                                "new_text": "Run integration and regression tests."
                            }]
                        }
                    },
                    "source": "mcp_store"
                }),
            ))
            .await;
        assert!(
            response.ok,
            "daemon rejected org text replacement envelope: {:?}",
            response.error
        );
        let stored: DaemonDraftOperationResponse =
            serde_json::from_value(response.payload).unwrap();

        let detail = service.get_draft(&stored.draft_id).await.unwrap();
        assert_eq!(detail.draft.scope, crate::DaemonDraftScope::Project);
        let source = detail.operations[0]
            .operation
            .create
            .as_ref()
            .unwrap()
            .content
            .org_source
            .as_ref()
            .unwrap();
        assert_eq!(source.resource_id, "rule_testing");
        assert_eq!(source.commit_id, "commit_test");

        let loaded = service
            .load_memory(LoadMemoryRequest {
                project_id: "prj_test".to_owned(),
                ids: vec![stored.draft_id.clone()],
                known_hashes: BTreeMap::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            loaded.resources[0].content.as_deref(),
            Some(
                "# Testing\n\nApply when changing retrieval.\n\nRun integration and regression tests.\n\nTags: testing"
            )
        );

        let discarded = service
            .dispatch(DaemonIpcRequest::new(
                "store_draft_operation",
                json!({
                    "project_id": "prj_test",
                    "scope": "org",
                    "resource": "memory",
                    "op": {
                        "discard": {
                            "id": stored.draft_id
                        }
                    },
                    "source": "mcp_store"
                }),
            ))
            .await;
        assert!(
            discarded.ok,
            "daemon rejected discarding an org draft: {:?}",
            discarded.error
        );
        let detail = service.get_draft(&stored.draft_id).await.unwrap();
        assert_eq!(
            detail.draft.status,
            crate::DaemonLocalDraftStatus::Discarded
        );

        let reverted = service
            .load_memory(LoadMemoryRequest {
                project_id: "prj_test".to_owned(),
                ids: vec!["rule_testing".to_owned()],
                known_hashes: BTreeMap::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            reverted.resources[0].content.as_deref(),
            original.content.as_deref()
        );
    }

    #[test]
    fn draft_overlay_uses_the_complete_base_result_for_every_crud_action() {
        fn context_resource(path: &str, content: &str, commit_id: &str) -> EffectiveResource {
            EffectiveResource {
                source: SourceResource {
                    resource_id: "ctx_target".to_owned(),
                    project_id: "prj_test".to_owned(),
                    scope: SourceScope::Project,
                    kind: MemoryKind::Memory,
                    path: path.to_owned(),
                    title: title_from_path(path),
                    description: String::new(),
                    content: content.to_owned(),
                    content_hash: sha256(content),
                    source_commit_id: Some(commit_id.to_owned()),
                    draft_id: None,
                    draft_revision: None,
                },
            }
        }

        fn overlay(
            draft_id: &str,
            target_id: Option<&str>,
            base_resource: Option<EffectiveResource>,
            operation: DaemonDraftOperation,
        ) -> super::overlay::DraftOverlay {
            super::overlay::DraftOverlay {
                draft_id: draft_id.to_owned(),
                base_commit_id: Some("commit_base".to_owned()),
                scope: SourceScope::Project,
                kind: Some(MemoryKind::Memory),
                target_id: target_id.map(ToOwned::to_owned),
                path: Some("context/base.md".to_owned()),
                base_resource,
                operations: vec![(1, serde_json::to_string(&operation).unwrap(), operation)],
            }
        }

        let base = context_resource("context/base.md", "# Base", "commit_base");
        let current = context_resource("context/base.md", "# Remote", "commit_current");

        let mut updated = BTreeMap::from([("ctx_target".to_owned(), current.clone())]);
        super::overlay::apply_draft_overlay(
            "prj_test",
            &mut updated,
            overlay(
                "draft_update",
                Some("ctx_target"),
                Some(base.clone()),
                DaemonDraftOperation {
                    create: None,
                    update: Some(DaemonUpdateDraftOperation::Content(
                        DaemonContentDraftUpdate {
                            id: "ctx_target".to_owned(),
                            content: DaemonDraftContent {
                                org_source: None,
                                description: None,
                                content: "# Personal Draft".to_owned(),
                            },
                            description: None,
                        },
                    )),
                    rename: None,
                    delete: None,
                    discard: None,
                },
            ),
        )
        .unwrap();
        assert_eq!(updated["ctx_target"].source.content, "# Personal Draft");
        assert_eq!(
            updated["ctx_target"].source.draft_id.as_deref(),
            Some("draft_update")
        );

        let mut renamed = BTreeMap::from([("ctx_target".to_owned(), current.clone())]);
        super::overlay::apply_draft_overlay(
            "prj_test",
            &mut renamed,
            overlay(
                "draft_rename",
                Some("ctx_target"),
                Some(base.clone()),
                DaemonDraftOperation {
                    create: None,
                    update: None,
                    rename: Some(DaemonRenameDraftOperation {
                        id: "ctx_target".to_owned(),
                        new_path: "context/renamed.md".to_owned(),
                        description: None,
                    }),
                    delete: None,
                    discard: None,
                },
            ),
        )
        .unwrap();
        assert_eq!(renamed["ctx_target"].source.path, "context/renamed.md");
        assert_eq!(renamed["ctx_target"].source.content, "# Base");

        let mut deleted = BTreeMap::from([("ctx_target".to_owned(), current.clone())]);
        super::overlay::apply_draft_overlay(
            "prj_test",
            &mut deleted,
            overlay(
                "draft_delete",
                Some("ctx_target"),
                Some(base),
                DaemonDraftOperation {
                    create: None,
                    update: None,
                    rename: None,
                    delete: Some(DaemonDeleteDraftOperation {
                        id: "ctx_target".to_owned(),
                        description: None,
                    }),
                    discard: None,
                },
            ),
        )
        .unwrap();
        assert!(!deleted.contains_key("ctx_target"));

        let mut created = BTreeMap::from([("ctx_target".to_owned(), current)]);
        super::overlay::apply_draft_overlay(
            "prj_test",
            &mut created,
            overlay(
                "draft_create",
                None,
                None,
                DaemonDraftOperation {
                    create: Some(DaemonCreateDraftOperation {
                        path: "context/new.md".to_owned(),
                        content: DaemonDraftContent {
                            org_source: None,
                            description: None,
                            content: "# New Draft".to_owned(),
                        },
                        description: None,
                    }),
                    update: None,
                    rename: None,
                    delete: None,
                    discard: None,
                },
            ),
        )
        .unwrap();
        assert_eq!(created["draft_create"].source.content, "# New Draft");
        assert!(created.contains_key("ctx_target"));
    }

    #[test]
    fn draft_overlay_revision_ignores_sync_only_timestamp_changes() {
        let operation = DaemonDraftOperation {
            create: Some(DaemonCreateDraftOperation {
                path: "context/new.md".to_owned(),
                content: DaemonDraftContent {
                    org_source: None,
                    description: None,
                    content: "# New context".to_owned(),
                },
                description: None,
            }),
            update: None,
            rename: None,
            delete: None,
            discard: None,
        };
        let operation_json = serde_json::to_string(&operation).unwrap();
        let make_overlay = || super::overlay::DraftOverlay {
            draft_id: "draft_timestamp".to_owned(),
            base_commit_id: Some("commit_base".to_owned()),
            scope: SourceScope::Project,
            kind: Some(MemoryKind::Memory),
            target_id: None,
            path: Some("context/new.md".to_owned()),
            base_resource: None,
            operations: vec![(1, operation_json.clone(), operation.clone())],
        };

        let mut before = BTreeMap::new();
        super::overlay::apply_draft_overlay("prj_test", &mut before, make_overlay()).unwrap();
        let mut after = BTreeMap::new();
        super::overlay::apply_draft_overlay("prj_test", &mut after, make_overlay()).unwrap();

        assert_eq!(
            before["draft_timestamp"].source.draft_revision,
            after["draft_timestamp"].source.draft_revision
        );
    }

    #[test]
    fn remote_create_draft_accepts_the_server_draft_id_as_a_provisional_alias() {
        let create = DaemonDraftOperation {
            create: Some(DaemonCreateDraftOperation {
                path: "context/new.md".to_owned(),
                content: DaemonDraftContent {
                    org_source: None,
                    description: Some("Semantic metadata".to_owned()),
                    content: "# Initial".to_owned(),
                },
                description: None,
            }),
            update: None,
            rename: None,
            delete: None,
            discard: None,
        };
        let update = DaemonDraftOperation {
            create: None,
            update: Some(DaemonUpdateDraftOperation::Content(
                DaemonContentDraftUpdate {
                    id: "draft_provisional".to_owned(),
                    content: DaemonDraftContent {
                        org_source: None,
                        description: None,
                        content: "# Updated".to_owned(),
                    },
                    description: None,
                },
            )),
            rename: None,
            delete: None,
            discard: None,
        };
        let rename = DaemonDraftOperation {
            create: None,
            update: None,
            rename: Some(DaemonRenameDraftOperation {
                id: "drf_server".to_owned(),
                new_path: "context/renamed.md".to_owned(),
                description: None,
            }),
            delete: None,
            discard: None,
        };
        let overlay = super::overlay::DraftOverlay {
            draft_id: "drf_server".to_owned(),
            base_commit_id: Some("commit_base".to_owned()),
            scope: SourceScope::Project,
            kind: Some(MemoryKind::Memory),
            target_id: None,
            path: Some("context/new.md".to_owned()),
            base_resource: None,
            operations: vec![
                (1, serde_json::to_string(&create).unwrap(), create),
                (2, serde_json::to_string(&update).unwrap(), update),
                (3, serde_json::to_string(&rename).unwrap(), rename),
            ],
        };

        let mut resources = BTreeMap::new();
        super::overlay::apply_draft_overlay("prj_test", &mut resources, overlay).unwrap();

        assert_eq!(resources["draft_provisional"].source.content, "# Updated");
        assert_eq!(
            resources["draft_provisional"].source.description,
            "Semantic metadata"
        );
        assert_eq!(
            resources["draft_provisional"].source.path,
            "context/renamed.md"
        );
        assert_eq!(
            resources["draft_provisional"].source.draft_id.as_deref(),
            Some("drf_server")
        );
        assert!(!resources.contains_key("drf_server"));
    }

    #[tokio::test]
    async fn failed_index_build_is_recorded_without_moving_the_search_head() {
        let (_temp, state) = test_state_with_models(Arc::new(FailingIndexModels)).await;
        let _worker = state.start_search_index_worker();
        scheduler::enqueue_project(&state, "prj_test")
            .await
            .unwrap();
        wait_for_index_job(&state, "failed").await;
        let error = state
            .activate_memory(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "hybrid".to_owned(),
                state: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DaemonError::Search { ref code, .. } if code == "search_index_failed"
        ));

        let status = state
            .search_index_status(SearchIndexProjectRequest {
                project_id: "prj_test".to_owned(),
            })
            .await
            .unwrap();
        assert!(!status.ready);
        assert!(status.active_revision.is_none());
        assert!(
            status
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("deterministic index failure"))
        );
        let runs = state
            .list_retrieval_runs(RetrievalRunListRequest {
                project_id: Some("prj_test".to_owned()),
                status: Some(RetrievalRunStatus::Failed),
                cursor: None,
                limit: Some(10),
            })
            .await
            .unwrap();
        assert_eq!(runs.items.len(), 1);
        assert_eq!(runs.items[0].error_stage.as_deref(), Some("index_head"));
        assert_eq!(
            runs.items[0].error_code.as_deref(),
            Some("search_index_failed")
        );
    }

    #[tokio::test]
    async fn retrieval_history_failure_does_not_change_activation_result() {
        let (_temp, state) = test_state().await;
        fs::write(
            state.inner.config.root_dir.join("evaluation-corpora"),
            "blocks corpus directory creation",
        )
        .unwrap();

        let response = state
            .activate_memory(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "hybrid".to_owned(),
                state: None,
            })
            .await
            .unwrap();
        assert!(!response.fragments.is_empty());

        let runs = state
            .list_retrieval_runs(RetrievalRunListRequest {
                project_id: Some("prj_test".to_owned()),
                status: Some(RetrievalRunStatus::Failed),
                cursor: None,
                limit: Some(10),
            })
            .await
            .unwrap();
        assert_eq!(runs.items.len(), 1);
        assert_eq!(runs.items[0].error_stage.as_deref(), Some("persistence"));
        assert_eq!(
            runs.items[0].error_code.as_deref(),
            Some("retrieval_history_persistence_failed")
        );
    }

    #[tokio::test]
    async fn preparing_models_report_progress_without_blocking_activation() {
        let (_temp, state) = test_state_with_models(Arc::new(PreparingModels)).await;
        let status = state
            .search_index_status(SearchIndexProjectRequest {
                project_id: "prj_test".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(status.model_status, SearchModelStatus::Preparing);
        assert_eq!(status.model_downloaded_bytes, Some(128));
        assert_eq!(status.model_total_bytes, Some(512));

        let error = state
            .activate_memory(ActivateMemoryRequest {
                project_id: "prj_test".to_owned(),
                query: "hybrid".to_owned(),
                state: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DaemonError::Search { ref code, .. } if code == "search_model_preparing"
        ));
    }

    #[test]
    fn index_build_bounds_embedding_batches_without_cloning_resources() {
        let content = (0..80)
            .map(|index| format!("# Section {index}\n\n{}\n", "x".repeat(500)))
            .collect::<String>();
        let resources = [SourceResource {
            resource_id: "ctx_large".to_owned(),
            project_id: "prj_test".to_owned(),
            scope: SourceScope::Project,
            kind: MemoryKind::Memory,
            path: "context/large.md".to_owned(),
            title: "Large".to_owned(),
            description: String::new(),
            content_hash: sha256(&content),
            content,
            source_commit_id: Some("commit_test".to_owned()),
            draft_id: None,
            draft_revision: None,
        }];
        let models = BatchRecordingModels {
            largest_batch: AtomicUsize::new(0),
            total_embedded: AtomicUsize::new(0),
        };
        let units = index::build_index_units(&resources, &models).unwrap();
        assert!(units.len() > index::INDEX_EMBED_BATCH_SIZE);
        assert!(models.largest_batch.load(Ordering::Relaxed) <= index::INDEX_EMBED_BATCH_SIZE);
        assert!(units.iter().all(|unit| unit.resource_index == 0));
    }

    #[tokio::test]
    async fn incremental_index_reuses_unchanged_resources_and_chunks_after_reopen() {
        let models = Arc::new(BatchRecordingModels {
            largest_batch: AtomicUsize::new(0),
            total_embedded: AtomicUsize::new(0),
        });
        let (temp, state) = test_state_with_models(models.clone()).await;
        let index_path = temp.path().join("incremental-index.sqlite");
        let pool = index::connect_project_index(&index_path).await.unwrap();
        let first_content = format!(
            "# Alpha\n\n{}\n\n# Beta\n\n{}\n",
            "a".repeat(900),
            "b".repeat(900)
        );
        let second_content = format!("# Gamma\n\n{}\n", "c".repeat(900));
        let first_resources: Arc<[SourceResource]> = vec![
            SourceResource {
                resource_id: "ctx_alpha".to_owned(),
                project_id: "prj_test".to_owned(),
                scope: SourceScope::Project,
                kind: MemoryKind::Memory,
                path: "context/alpha.md".to_owned(),
                title: "Alpha".to_owned(),
                description: String::new(),
                content_hash: sha256(&first_content),
                content: first_content.clone(),
                source_commit_id: Some("commit_one".to_owned()),
                draft_id: None,
                draft_revision: None,
            },
            SourceResource {
                resource_id: "ctx_gamma".to_owned(),
                project_id: "prj_test".to_owned(),
                scope: SourceScope::Project,
                kind: MemoryKind::Memory,
                path: "context/gamma.md".to_owned(),
                title: "Gamma".to_owned(),
                description: String::new(),
                content_hash: sha256(&second_content),
                content: second_content,
                source_commit_id: Some("commit_one".to_owned()),
                draft_id: None,
                draft_revision: None,
            },
        ]
        .into();
        let first = EffectiveMemory {
            project_id: "prj_test".to_owned(),
            effective_hash: "effective_one".to_owned(),
            resources: first_resources,
        };
        let prepared =
            match index::prepare_incremental_index(&state, &pool, &first, || async { true })
                .await
                .unwrap()
            {
                index::PrepareIndexOutcome::Prepared(prepared) => prepared,
                other => panic!("expected cold prepared index, got {other:?}"),
            };
        let cold_count = models.total_embedded.load(Ordering::Relaxed);
        assert!(cold_count > 1);
        let revision = index::stage_prepared_index(&pool, &first, &prepared)
            .await
            .unwrap();
        index::publish_staged_index(&pool, "prj_test", &first.effective_hash, &revision)
            .await
            .unwrap();

        // Reopen the on-disk database to prove reuse is persisted rather than
        // an in-memory optimization. A same-length edit changes exactly one
        // chunk; every other chunk and the untouched resource must be reused.
        pool.close().await;
        let pool = index::connect_project_index(&index_path).await.unwrap();
        models.total_embedded.store(0, Ordering::Relaxed);
        let mut changed_resources = first.resources.to_vec();
        let mut changed_content = first_content.clone();
        let body_start = changed_content.find("\n\n").unwrap() + 2;
        changed_content.replace_range(body_start..body_start + 1, "z");
        changed_resources[0].content = changed_content;
        changed_resources[0].content_hash = sha256(&changed_resources[0].content);
        changed_resources[0].source_commit_id = Some("commit_two".to_owned());
        let changed = EffectiveMemory {
            project_id: "prj_test".to_owned(),
            effective_hash: "effective_two".to_owned(),
            resources: changed_resources.into(),
        };
        let prepared =
            match index::prepare_incremental_index(&state, &pool, &changed, || async { true })
                .await
                .unwrap()
            {
                index::PrepareIndexOutcome::Prepared(prepared) => prepared,
                other => panic!("expected changed prepared index, got {other:?}"),
            };
        assert_eq!(models.total_embedded.load(Ordering::Relaxed), 1);
        let revision = index::stage_prepared_index(&pool, &changed, &prepared)
            .await
            .unwrap();
        index::publish_staged_index(&pool, "prj_test", &changed.effective_hash, &revision)
            .await
            .unwrap();

        models.total_embedded.store(0, Ordering::Relaxed);
        assert!(matches!(
            index::prepare_incremental_index(&state, &pool, &changed, || async { true })
                .await
                .unwrap(),
            index::PrepareIndexOutcome::AlreadyReady(_)
        ));
        assert_eq!(models.total_embedded.load(Ordering::Relaxed), 0);
        pool.close().await;
    }

    #[test]
    fn fragment_selection_suppresses_overlap_and_irrelevant_tail() {
        let mut candidates = vec![
            ranked_test_row("best", "same", 0, 200, 300, 2.0),
            ranked_test_row("overlap", "same", 150, 350, 300, 1.0),
            ranked_test_row("distinct", "same", 400, 600, 300, 0.0),
            ranked_test_row("irrelevant", "other", 0, 100, 100, -10.0),
        ];
        for (index, candidate) in candidates.iter_mut().enumerate() {
            candidate.reranker_rank = Some(index + 1);
        }
        query::apply_fragment_budget(&mut candidates);

        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.final_rank.is_some())
                .map(|candidate| candidate.row.unit_key.as_str())
                .collect::<Vec<_>>(),
            vec!["best", "distinct"]
        );
        assert_eq!(
            candidates[1].exclusion_reason,
            RetrievalExclusionReason::Overlap
        );
        assert_eq!(
            candidates[3].exclusion_reason,
            RetrievalExclusionReason::BelowRelevance
        );
        assert!((query::rerank_relevance(0.0) - 0.5).abs() < f32::EPSILON);
        assert!(query::rerank_relevance(-10.0) < MIN_RERANK_RELEVANCE);
    }

    #[test]
    fn fragment_selection_does_not_fill_budget_with_lower_ranked_noise() {
        let mut candidates = vec![
            ranked_test_row("large", "one", 0, 100, 2_200, 2.0),
            ranked_test_row("overflow", "two", 0, 100, 300, 1.0),
            ranked_test_row("filler", "three", 0, 100, 100, 0.0),
        ];
        for (index, candidate) in candidates.iter_mut().enumerate() {
            candidate.reranker_rank = Some(index + 1);
        }
        query::apply_fragment_budget(&mut candidates);
        let selected = candidates
            .iter()
            .filter(|candidate| candidate.final_rank.is_some())
            .collect::<Vec<_>>();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].row.unit_key, "large");
        assert_eq!(
            candidates[1].exclusion_reason,
            RetrievalExclusionReason::TokenBudget
        );
        assert_eq!(
            candidates[2].exclusion_reason,
            RetrievalExclusionReason::TokenBudget
        );
    }

    #[test]
    fn fragment_selection_records_resource_and_fragment_limits() {
        let mut per_resource = vec![
            ranked_test_row("one", "same", 0, 10, 10, 2.0),
            ranked_test_row("two", "same", 20, 30, 10, 2.0),
            ranked_test_row("three", "same", 40, 50, 10, 2.0),
        ];
        for (index, candidate) in per_resource.iter_mut().enumerate() {
            candidate.reranker_rank = Some(index + 1);
        }
        query::apply_fragment_budget(&mut per_resource);
        assert_eq!(
            per_resource[2].exclusion_reason,
            RetrievalExclusionReason::PerResourceLimit
        );

        let mut fragment_limit = (0..=query::FINAL_FRAGMENTS)
            .map(|index| {
                ranked_test_row(
                    &format!("unit-{index}"),
                    &format!("resource-{index}"),
                    0,
                    10,
                    10,
                    2.0,
                )
            })
            .collect::<Vec<_>>();
        for (index, candidate) in fragment_limit.iter_mut().enumerate() {
            candidate.reranker_rank = Some(index + 1);
        }
        query::apply_fragment_budget(&mut fragment_limit);
        assert_eq!(
            fragment_limit[query::FINAL_FRAGMENTS].exclusion_reason,
            RetrievalExclusionReason::FragmentLimit
        );
    }

    #[test]
    #[ignore = "loads the pinned models and builds 6,250 real embeddings"]
    fn quantized_models_build_large_index_with_bounded_batches() {
        let cache_dir = std::env::var_os("CLUMSIES_MODEL_TEST_CACHE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| tempfile::tempdir().unwrap().keep());
        let models = FastEmbedSearchModels::new(cache_dir);
        models.prepare().unwrap();

        let content = (0..6_250)
            .map(|index| {
                format!(
                    "## Memory {index}\n\nmemory delta uses a content hash so the same memory is not injected twice.\n\n"
                )
            })
            .collect::<String>();
        let resources = [SourceResource {
            resource_id: "ctx_large_real".to_owned(),
            project_id: "prj_test".to_owned(),
            scope: SourceScope::Project,
            kind: MemoryKind::Memory,
            path: "context/large-real.md".to_owned(),
            title: "Large real index".to_owned(),
            description: String::new(),
            content_hash: sha256(&content),
            content,
            source_commit_id: Some("commit_test".to_owned()),
            draft_id: None,
            draft_revision: None,
        }];

        let started = std::time::Instant::now();
        let units = index::build_index_units(&resources, &models).unwrap();
        // Diagnostic timing for index build benchmark
        eprintln!(
            "built {} real embedding units in {:.2?}",
            units.len(),
            started.elapsed()
        );
        assert_eq!(units.len(), 6_250);
    }

    #[test]
    fn fts_builder_never_exposes_user_syntax() {
        let expression = query::fts_expression("混合检索 OR \"secret\" path/file.rs").unwrap();
        assert!(expression.contains("\"混合检\""));
        assert!(expression.contains("\"path/file.rs\""));
        assert!(
            expression
                .split(" OR ")
                .all(|term| term.starts_with('"') && term.ends_with('"'))
        );
    }

    #[test]
    fn activation_state_rejects_corruption_and_duplicate_identities() {
        assert!(activation::decode_activation_state(Some("not-base64!")).is_err());
        let duplicate = activation::ActivationStateToken {
            version: 1,
            epoch: 1,
            known: vec![
                activation::KnownActivationIdentity {
                    unit_key: "same".to_owned(),
                    content_hash: "one".to_owned(),
                    last_seen: 1,
                },
                activation::KnownActivationIdentity {
                    unit_key: "same".to_owned(),
                    content_hash: "two".to_owned(),
                    last_seen: 1,
                },
            ],
        };
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&duplicate).unwrap());
        assert!(activation::decode_activation_state(Some(&encoded)).is_err());
    }

    #[test]
    fn retrieval_error_maps_deadline_and_search_codes_to_stable_error_strings() {
        let (code, _) = retrieval_error(&DaemonError::Search {
            code: "activation_deadline".to_owned(),
            message: "exceeded budget".to_owned(),
        });
        assert_eq!(code, "activation_deadline");

        let (code, _) = retrieval_error(&DaemonError::Search {
            code: "search_model_preparing".to_owned(),
            message: "models preparing".to_owned(),
        });
        assert_eq!(code, "search_model_preparing");

        let (code, _) = retrieval_error(&DaemonError::InvalidRequest("bad".to_owned()));
        assert_eq!(code, "invalid_request");
    }
}
